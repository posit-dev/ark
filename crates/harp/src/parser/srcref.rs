//
// srcref.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use core::f64;

use anyhow::anyhow;
use stdext::unwrap;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::vector::IntegerVector;
use crate::vector::Vector;
use crate::RObject;

/// Structured representation of `srcref` integer vectors
/// 0-based offsets.
#[derive(Debug)]
pub struct SrcRef {
    /// Lines and virtual lines may differ if a `#line` directive is used in code:
    /// the former just counts actual lines, the latter respects the directive.
    /// `line` corresponds to `line_parsed` in the original base R srcref vector.
    pub line: std::ops::Range<u32>,
    pub line_virtual: std::ops::Range<u32>,

    /// Bytes and columns may be different due to multibyte characters.
    pub column: std::ops::Range<u32>,
    pub column_byte: std::ops::Range<u32>,
}

#[derive(Debug)]
pub struct SrcFile {
    pub inner: RObject,
}

// Takes user-facing object as input. The srcrefs are retrieved from
// attributes.
impl RObject {
    pub fn srcrefs(&self) -> anyhow::Result<Vec<SrcRef>> {
        let srcref = unwrap!(self.attr("srcref"), None => {
            return Err(anyhow!("Can't find `srcref` attribute"));
        });

        crate::List::new(srcref.sexp)?
            .iter()
            .map(|x| SrcRef::try_from(RObject::view(x)))
            .collect()
    }
}

// Takes individual `srcref` attribute as input
impl TryFrom<RObject> for SrcRef {
    type Error = anyhow::Error;

    fn try_from(value: RObject) -> anyhow::Result<Self> {
        crate::r_assert_type(value.sexp, &[libr::INTSXP])?;
        crate::r_assert_capacity(value.sexp, 6)?;

        let value = IntegerVector::new(value)?;

        // The srcref values are adjusted to produce a `[ )` range as expected
        // by `std::ops::Range` that counts from 0. This is in contrast to the
        // ranges in `srcref` vectors which are 1-based `[ ]`.

        // Change from 1-based to 0-based counting
        let adjust_start = |i| (i - 1) as u32;

        // Change from 1-based to 0-based counting (-1) and make it an exclusive
        // boundary (+1). So essentially a no-op.
        let adjust_end = |i| i as u32;

        let line_start = adjust_start(value.get_value(0)?);
        let column_start = adjust_start(value.get_value(4)?);
        let column_byte_start = adjust_start(value.get_value(1)?);

        let line_end = adjust_end(value.get_value(2)?);
        let column_end = adjust_end(value.get_value(5)?);
        let column_byte_end = adjust_end(value.get_value(3)?);

        let line = std::ops::Range {
            start: line_start,
            end: line_end,
        };

        let line_parsed = if unsafe { value.len() >= 8 } {
            let line_parsed_start = adjust_start(value.get_value(6)?);
            let line_parsed_end = adjust_end(value.get_value(7)?);
            std::ops::Range {
                start: line_parsed_start,
                end: line_parsed_end,
            }
        } else {
            line.clone()
        };

        let column = std::ops::Range {
            start: column_start,
            end: column_end,
        };

        let column_byte = std::ops::Range {
            start: column_byte_start,
            end: column_byte_end,
        };

        Ok(Self {
            line: line_parsed,
            line_virtual: line,
            column,
            column_byte,
        })
    }
}

/// Creates the same sort of srcfile object as with `parse(text = )`.
/// Takes code as an R string containing newlines, or as a R vector of lines.
impl SrcFile {
    fn new_virtual(text: RObject) -> harp::Result<Self> {
        let inner = RFunction::new("base", "srcfilecopy")
            .param("filename", "<text>")
            .param("lines", text)
            .call()?;

        Ok(Self { inner })
    }

    pub fn lines(&self) -> harp::Result<RObject> {
        RFunction::new("base", "getSrcLines")
            .add(self.inner.sexp)
            .param("first", 1)
            .param("last", f64::INFINITY)
            .call()
    }
}

impl TryFrom<&str> for SrcFile {
    type Error = harp::Error;

    fn try_from(value: &str) -> harp::Result<Self> {
        let input = crate::as_parse_text(value);
        SrcFile::new_virtual(input)
    }
}

impl TryFrom<&harp::CharacterVector> for SrcFile {
    type Error = harp::Error;

    fn try_from(value: &harp::CharacterVector) -> harp::Result<Self> {
        SrcFile::new_virtual(value.object.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use crate::fixtures::r_task;
    use crate::srcref::SrcRef;

    #[test]
    fn test_srcref() {
        r_task(|| {
            let exprs = crate::parse_exprs_with_srcrefs("foo\n\nś\nbar(\n\n)").unwrap();
            let srcrefs: Vec<SrcRef> = exprs.srcrefs().unwrap();
            let foo = &srcrefs[0];
            let utf8 = &srcrefs[1];
            let bar = &srcrefs[2];

            assert_eq!(foo.line, Range { start: 0, end: 1 });
            assert_eq!(foo.line_virtual, Range { start: 0, end: 1 });
            assert_eq!(foo.column, Range { start: 0, end: 3 });
            assert_eq!(foo.column_byte, Range { start: 0, end: 3 });

            // `column_byte` is different because the character takes up two bytes
            assert_eq!(utf8.line, Range { start: 2, end: 3 });
            assert_eq!(utf8.line_virtual, Range { start: 2, end: 3 });
            assert_eq!(utf8.column, Range { start: 0, end: 1 });
            assert_eq!(utf8.column_byte, Range { start: 0, end: 2 });

            // Ends on different lines
            assert_eq!(bar.line, Range { start: 3, end: 6 });
            assert_eq!(bar.line_virtual, Range { start: 3, end: 6 });
            assert_eq!(bar.column, Range { start: 0, end: 1 });
            assert_eq!(bar.column_byte, Range { start: 0, end: 1 });
        })
    }

    #[test]
    fn test_srcref_line_directive() {
        r_task(|| {
            let exprs = crate::parse_exprs_with_srcrefs("foo\n#line 5\nbar").unwrap();
            let srcrefs: Vec<SrcRef> = exprs.srcrefs().unwrap();
            let foo = &srcrefs[0];
            let bar = &srcrefs[1];

            assert_eq!(foo.line, Range { start: 0, end: 1 });
            assert_eq!(foo.line_virtual, Range { start: 0, end: 1 });

            // Custom line via directive
            assert_eq!(bar.line, Range { start: 2, end: 3 });
            assert_eq!(bar.line_virtual, Range { start: 4, end: 5 });
        })
    }
}
