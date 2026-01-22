//
// document.rs
//
// Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
//
//

use aether_lsp_utils::proto::from_proto;
use aether_lsp_utils::proto::PositionEncoding;
use tower_lsp::lsp_types;
use tree_sitter::Parser;
use tree_sitter::Tree;

use crate::lsp::config::DocumentConfig;

#[derive(Clone)]
pub struct Document {
    /// The document's textual contents.
    pub contents: String,

    /// The document's AST.
    pub ast: Tree,

    /// The Rowan R syntax tree.
    pub parse: aether_parser::Parse,

    /// Index of new lines and non-UTF-8 characters in `contents`. Used for converting
    /// between line/col [tower_lsp::Position]s with a specified [PositionEncoding] to
    /// [biome_text_size::TextSize] offsets.
    pub line_index: biome_line_index::LineIndex,

    /// The version of the document we last synchronized with.
    /// None if the document hasn't been synchronized yet.
    pub version: Option<i32>,

    /// Position encoding used for LSP position conversions.
    pub position_encoding: PositionEncoding,

    /// Configuration of the document, such as indentation settings.
    pub config: DocumentConfig,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("contents", &self.contents)
            .field("ast", &self.ast)
            .field("parse", &self.parse)
            .finish()
    }
}

impl Document {
    pub fn new(contents: &str, version: Option<i32>) -> Self {
        // A one-shot parser, assumes the `Document` won't be incrementally reparsed.
        // Useful for testing, `with_document()`, and `index_file()`.
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .unwrap();

        Self::new_with_parser(contents, &mut parser, version)
    }

    pub fn new_with_parser(contents: &str, parser: &mut Parser, version: Option<i32>) -> Self {
        let contents = String::from(contents);

        // Legacy Tree-Sitter AST
        let ast = parser.parse(contents.as_str(), None).unwrap();

        // Preferred Rowan AST and accompanying line index
        let parse = aether_parser::parse(&contents, Default::default());
        let line_index = biome_line_index::LineIndex::new(&contents);

        Self {
            contents,
            version,
            ast,
            parse,
            line_index,
            // Currently hard-coded to UTF-16, but we might want to allow UTF-8 frontends
            // once/if Ark becomes an independent LSP
            position_encoding: PositionEncoding::Wide(biome_line_index::WideEncoding::Utf16),
            config: Default::default(),
        }
    }

    // --- source
    // authors = ["rust-analyzer team"]
    // license = "MIT OR Apache-2.0"
    // origin = "https://github.com/rust-lang/rust-analyzer/blob/master/crates/rust-analyzer/src/lsp/utils.rs"
    // ---
    pub fn on_did_change(
        &mut self,
        parser: &mut Parser,
        params: &lsp_types::DidChangeTextDocumentParams,
    ) {
        let new_version = params.text_document.version;

        // Check for out-of-order change notifications
        if let Some(old_version) = self.version {
            // According to the spec, versions might not be consecutive but they must be monotonically
            // increasing. If that's not the case this is a hard nope as we
            // can't maintain our state integrity. Currently panicking but in
            // principle we should shut down the LSP in an orderly fashion.
            if new_version < old_version {
                panic!(
                    "out-of-sync change notification: currently at {old_version}, got {new_version}"
                );
            }
        }

        let mut changes = params.content_changes.clone();

        // If at least one of the changes is a full document change, use the last of them
        // as the starting point and ignore all previous changes. We then know that all
        // changes after this (if any!) are incremental changes.
        //
        // If we do have a full document change, that implies the `last_start_line`
        // corresponding to that change is line 0, which will correctly force a rebuild
        // of the line index before applying any incremental changes. We don't go ahead
        // and rebuild the line index here, because it is guaranteed to be rebuilt for
        // us on the way out.
        let (changes, mut last_start_line) =
            match changes.iter().rposition(|change| change.range.is_none()) {
                Some(idx) => {
                    let incremental = changes.split_off(idx + 1);
                    // Unwrap: `rposition()` confirmed this index contains a full document change
                    let change = changes.pop().unwrap();
                    self.contents = change.text;
                    (incremental, 0)
                },
                None => (changes, u32::MAX),
            };

        // Handle all incremental changes after the last full document change. We don't
        // typically get >1 incremental change as the user types, but we do get them in a
        // batch after a find-and-replace, or after a format-on-save request.
        //
        // Some editors like VS Code send the edits in reverse order (from the bottom of
        // file -> top of file). We can take advantage of this, because applying an edit
        // on, say, line 10, doesn't invalidate the `line_index` if we then need to apply
        // an additional edit on line 5. That said, we may still have edits that cross
        // lines, so rebuilding the `line_index` is not always unavoidable.
        for change in changes {
            let range = change
                .range
                .expect("`None` case already handled by finding the last full document change.");

            // If the end of this change is at or past the start of the last change, then
            // the `line_index` needed to apply this change is now invalid, so we have to
            // rebuild it.
            if range.end.line >= last_start_line {
                self.line_index = biome_line_index::LineIndex::new(&self.contents);
            }
            last_start_line = range.start.line;

            // This is a panic if we can't convert. It means we can't keep the document up
            // to date and something is very wrong.
            let range: std::ops::Range<usize> =
                from_proto::text_range(range, &self.line_index, self.position_encoding)
                    .expect("Can convert `range` from `Position` to `TextRange`.")
                    .into();

            self.contents.replace_range(range, &change.text);
        }

        // Rebuild everything once at the end
        self.line_index = biome_line_index::LineIndex::new(&self.contents);
        self.parse = aether_parser::parse(&self.contents, Default::default());
        self.ast = parser.parse(self.contents.as_str(), None).unwrap();
        self.version = Some(new_version);
    }

    pub fn get_line(&self, line: usize) -> Option<&str> {
        let Some(line_start) = self.line_index.newlines.get(line) else {
            // Forcing a full capture so we can learn the situations in which this occurs
            log::error!(
                "Requesting line {line} but only {n} lines exist.\n\nDocument:\n{contents}\n\nBacktrace:\n{trace}",
                n = self.line_index.len(),
                line = line + 1,
                contents = &self.contents,
                trace = std::backtrace::Backtrace::force_capture(),
            );
            return None;
        };

        let line_end = self
            .line_index
            .newlines
            .get(line + 1)
            .copied()
            // if `line` is last, extract text until end of buffer
            .unwrap_or_else(|| (self.contents.len() as u32).into());

        let line_start_byte: usize = line_start.to_owned().into();
        let line_end_byte: usize = line_end.into();

        self.contents.get(line_start_byte..line_end_byte)
    }

    /// Accessor that returns an annotated `RSyntaxNode` type.
    /// More convenient than the generic `biome_rowan::SyntaxNode<L>` type.
    pub fn syntax(&self) -> aether_syntax::RSyntaxNode {
        self.parse.syntax()
    }

    pub fn tree_sitter_point_from_lsp_position(
        &self,
        position: lsp_types::Position,
    ) -> anyhow::Result<tree_sitter::Point> {
        let offset = from_proto::offset(position, &self.line_index, self.position_encoding)?;
        let line_col = self.line_index.line_col(offset).ok_or_else(|| {
            anyhow::anyhow!("Failed to convert LSP position {position:?} to LineCol offset")
        })?;
        Ok(tree_sitter::Point::new(
            line_col.line as usize,
            line_col.col as usize,
        ))
    }

    pub fn lsp_position_from_tree_sitter_point(
        &self,
        point: tree_sitter::Point,
    ) -> anyhow::Result<lsp_types::Position> {
        let line_col = biome_line_index::LineCol {
            line: point.row as u32,
            col: point.column as u32,
        };

        match self.position_encoding {
            PositionEncoding::Utf8 => Ok(lsp_types::Position::new(line_col.line, line_col.col)),
            PositionEncoding::Wide(wide_encoding) => {
                let wide_line_col = self
                    .line_index
                    .to_wide(wide_encoding, line_col)
                    .ok_or_else(|| {
                        anyhow::anyhow!("Failed to convert Tree-Sitter point {point:?} to wide line column for document")
                    })?;
                Ok(lsp_types::Position::new(
                    wide_line_col.line as u32,
                    wide_line_col.col as u32,
                ))
            },
        }
    }

    pub fn lsp_range_from_tree_sitter_range(
        &self,
        range: tree_sitter::Range,
    ) -> anyhow::Result<lsp_types::Range> {
        let start = self.lsp_position_from_tree_sitter_point(range.start_point)?;
        let end = self.lsp_position_from_tree_sitter_point(range.end_point)?;
        Ok(lsp_types::Range::new(start, end))
    }

    pub fn tree_sitter_range_from_lsp_range(
        &self,
        range: lsp_types::Range,
    ) -> anyhow::Result<tree_sitter::Range> {
        let start_point = self.tree_sitter_point_from_lsp_position(range.start)?;
        let end_point = self.tree_sitter_point_from_lsp_position(range.end)?;

        let start_offset =
            from_proto::offset(range.start, &self.line_index, self.position_encoding)?;
        let end_offset = from_proto::offset(range.end, &self.line_index, self.position_encoding)?;

        Ok(tree_sitter::Range {
            start_byte: start_offset.into(),
            end_byte: end_offset.into(),
            start_point,
            end_point,
        })
    }
}

#[cfg(test)]
mod tests {
    use tree_sitter::Point;

    use super::*;

    #[test]
    fn test_document_starts_at_0_0_with_leading_whitespace() {
        let document = Document::new("\n\n# hi there", None);
        let root = document.ast.root_node();
        assert_eq!(root.start_position(), Point::new(0, 0));
    }

    #[test]
    fn test_aether_syntax_integration() {
        let document = Document::new("foo <- 1 + 2", None);

        let syntax = document.parse.syntax();
        let len: u32 = syntax.text_range_with_trivia().len().into();
        assert!(len > 0);

        let syntax2 = document.syntax();
        assert_eq!(
            syntax.text_range_with_trivia(),
            syntax2.text_range_with_trivia()
        );

        assert!(!document.parse.has_error());
    }

    #[test]
    fn test_tree_sitter_point_from_lsp_position_wide_encoding() {
        // The emoji is 4 UTF-8 bytes and 2 UTF-16 bytes
        let mut document = Document::new("😃a", None);
        document.position_encoding = PositionEncoding::Wide(biome_line_index::WideEncoding::Utf16);

        let point = document
            .tree_sitter_point_from_lsp_position(lsp_types::Position::new(0, 2))
            .unwrap();
        assert_eq!(point, Point::new(0, 4));

        let point = document
            .tree_sitter_point_from_lsp_position(lsp_types::Position::new(0, 3))
            .unwrap();
        assert_eq!(point, Point::new(0, 5));
    }

    #[test]
    fn test_lsp_position_from_tree_sitter_point_wide_encoding() {
        let mut document = Document::new("😃a", None);
        document.position_encoding = PositionEncoding::Wide(biome_line_index::WideEncoding::Utf16);

        let position = document
            .lsp_position_from_tree_sitter_point(Point::new(0, 4))
            .unwrap();
        assert_eq!(position, lsp_types::Position::new(0, 2));

        let position = document
            .lsp_position_from_tree_sitter_point(Point::new(0, 5))
            .unwrap();
        assert_eq!(position, lsp_types::Position::new(0, 3));
    }

    #[test]
    fn test_utf8_position_roundtrip_multibyte() {
        // `é` is 2 bytes
        let mut document = Document::new("é\n", None);
        document.position_encoding = PositionEncoding::Utf8;

        let lsp_position = lsp_types::Position::new(0, 2);
        let point = document
            .tree_sitter_point_from_lsp_position(lsp_position)
            .unwrap();
        assert_eq!(point, Point::new(0, 2));

        let roundtrip_position = document.lsp_position_from_tree_sitter_point(point).unwrap();
        assert_eq!(roundtrip_position, lsp_position);
    }

    // After an incremental update, the AST reflects the new document contents,
    // not the old ones
    #[test]
    fn test_incremental_update_keeps_ast_in_sync() {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .unwrap();

        let mut document = Document::new_with_parser("", &mut parser, Some(1));
        assert_eq!(document.contents, "");
        assert_eq!(document.ast.root_node().end_position(), Point::new(0, 0));

        // Simulate typing "lib" character by character
        let changes = [
            (
                "l",
                lsp_types::Range::new(
                    lsp_types::Position::new(0, 0),
                    lsp_types::Position::new(0, 0),
                ),
            ),
            (
                "i",
                lsp_types::Range::new(
                    lsp_types::Position::new(0, 1),
                    lsp_types::Position::new(0, 1),
                ),
            ),
            (
                "b",
                lsp_types::Range::new(
                    lsp_types::Position::new(0, 2),
                    lsp_types::Position::new(0, 2),
                ),
            ),
        ];

        for (i, (text, range)) in changes.iter().enumerate() {
            let params = lsp_types::DidChangeTextDocumentParams {
                text_document: lsp_types::VersionedTextDocumentIdentifier {
                    uri: lsp_types::Url::parse("file:///test.R").unwrap(),
                    version: (i + 2) as i32,
                },
                content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                    range: Some(*range),
                    range_length: None,
                    text: text.to_string(),
                }],
            };
            document.on_did_change(&mut parser, &params);
        }

        // After typing "lib", document should contain "lib"
        assert_eq!(document.contents, "lib");

        // The AST should reflect the current contents, not be one edit behind.
        // The root node should span the entire "lib" identifier.
        let root = document.ast.root_node();
        assert_eq!(root.end_position(), Point::new(0, 3));

        // Verify we can find a node at position (0, 3) which is at the end of "lib"
        use crate::lsp::traits::node::NodeExt;
        let node = root.find_smallest_spanning_node(Point::new(0, 3));
        assert!(node.is_some(), "Should find spanning node at end of 'lib'");

        // The Rowan tree contains the updated document
        assert_eq!(document.syntax().text_with_trivia(), "lib");
    }
}
