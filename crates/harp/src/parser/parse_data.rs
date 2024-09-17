use crate::exec::RFunction;
use crate::exec::RFunctionExt;

#[derive(Clone, Debug)]
pub struct ParseData {
    pub nodes: Vec<ParseDataNode>,
}

/// `text` is not included because long strings are not stored by the
/// parser.
#[derive(Clone, Debug)]
pub struct ParseDataNode {
    /// Unlike `SrcRef`, parse data nodes don't include virtual line information
    /// created by `#line` directives. 0-based `[ )` range.
    pub line: std::ops::Range<u32>,

    /// Parse data nodes only contain column offset according to the text
    /// encoding used by the parser. 0-based `[ )` range.
    pub column: std::ops::Range<u32>,

    /// 0-based indices into the storage vector.
    pub id: u32,
    pub parent: u32,

    /// Node kind.
    pub kind: ParseDataKind,
}

#[derive(Clone, Debug)]
pub enum ParseDataKind {
    Node,
    Token(String),
}

impl ParseData {
    pub fn from_srcfile(srcfile: &harp::srcref::SrcFile) -> harp::Result<Self> {
        let data = RFunction::new("utils", "getParseData")
            .add(srcfile.inner.sexp)
            .call()?;

        if data.sexp == harp::RObject::null().sexp {
            return Err(harp::anyhow!("Can't find parse data in srcfile"));
        }

        let data = harp::DataFrame::new(data.sexp)?;

        let mut nodes: Vec<ParseDataNode> = Vec::with_capacity(data.nrow);

        let line1: Vec<i32> = (&data.col("line1")?).try_into()?;
        let line2: Vec<i32> = (&data.col("line2")?).try_into()?;
        let col1: Vec<i32> = (&data.col("col1")?).try_into()?;
        let col2: Vec<i32> = (&data.col("col2")?).try_into()?;
        let id: Vec<i32> = (&data.col("id")?).try_into()?;
        let parent: Vec<i32> = (&data.col("parent")?).try_into()?;
        let token: Vec<String> = (&data.col("token")?).try_into()?;
        let terminal: Vec<bool> = (&data.col("terminal")?).try_into()?;

        // The srcref values are adjusted to produce a `[ )` range as expected
        // by `std::ops::Range` that counts from 0. This is in contrast to the
        // ranges in `srcref` vectors which are 1-based `[ ]`.

        // Change from 1-based to 0-based counting
        let adjust_start = |i| (i - 1) as u32;

        // Change from 1-based to 0-based counting (-1) and make it an exclusive
        // boundary (+1). So essentially a no-op.
        let adjust_end = |i| i as u32;

        let row_iter = itertools::izip!(
            line1.into_iter(),
            line2.into_iter(),
            col1.into_iter(),
            col2.into_iter(),
            id.into_iter(),
            parent.into_iter(),
            token.into_iter(),
            terminal.into_iter(),
        );

        for row in row_iter {
            let line1 = row.0;
            let line2 = row.1;
            let col1 = row.2;
            let col2 = row.3;
            let id = row.4;
            let parent = row.5;
            let token = row.6;
            let terminal = row.7;

            let node = ParseDataNode {
                line: std::ops::Range {
                    start: adjust_start(line1),
                    end: adjust_end(line2),
                },
                column: std::ops::Range {
                    start: adjust_start(col1),
                    end: adjust_end(col2),
                },
                id: id as u32,
                parent: parent as u32,
                kind: if terminal {
                    ParseDataKind::Token(token)
                } else {
                    ParseDataKind::Node
                },
            };

            nodes.push(node);
        }

        Ok(Self { nodes })
    }

    pub fn filter_top_level(mut self) -> Self {
        let nodes = std::mem::take(&mut self.nodes);
        self.nodes = nodes
            .into_iter()
            .filter(|node| matches!(node.kind, ParseDataKind::Node) && node.parent == 0)
            .collect();
        self
    }
}

impl ParseDataNode {
    pub fn as_point_range(&self) -> std::ops::Range<(u32, u32)> {
        std::ops::Range {
            start: (self.line.start, self.column.start),
            end: (self.line.end, self.column.end),
        }
    }
}

impl harp::srcref::SrcFile {
    pub fn parse_data(&self) -> harp::Result<ParseData> {
        ParseData::from_srcfile(self)
    }
}

#[cfg(test)]
mod tests {
    use harp::parse;
    use harp::srcref;

    use crate::parse_data::ParseData;
    use crate::test::r_test;

    #[test]
    fn test_parse_data() {
        r_test(|| {
            let srcfile = srcref::SrcFile::try_from("foo\nbar").unwrap();
            let exprs = parse::parse_exprs_ext(&parse::ParseInput::SrcFile(&srcfile)).unwrap();
            let srcrefs: Vec<harp::srcref::SrcRef> = exprs.srcrefs().unwrap();

            let parse_data = ParseData::from_srcfile(&srcfile).unwrap();
            let top_level = parse_data.filter_top_level();

            let parse_first = top_level.nodes.get(0).unwrap();
            let parse_second = top_level.nodes.get(1).unwrap();
            let srcref_first = srcrefs.get(0).unwrap();
            let srcref_second = srcrefs.get(1).unwrap();

            assert_eq!(parse_first.line, srcref_first.line);
            assert_eq!(parse_first.column, srcref_first.column);

            assert_eq!(parse_second.line, srcref_second.line);
            assert_eq!(parse_second.column, srcref_second.column);
        })
    }
}
