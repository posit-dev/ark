//
// document.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use tower_lsp::lsp_types::DidChangeTextDocumentParams;
use tower_lsp::lsp_types::PositionEncodingKind;
use tower_lsp::lsp_types::TextDocumentContentChangeEvent;
use tree_sitter::InputEdit;
use tree_sitter::Parser;
use tree_sitter::Point;
use tree_sitter::Tree;

use crate::lsp::config::DocumentConfig;
use crate::lsp::encoding::convert_lsp_range_to_tree_sitter_range;

fn compute_point(point: Point, text: &str) -> Point {
    // figure out where the newlines in this edit are
    let newline_indices: Vec<_> = text.match_indices('\n').collect();
    let num_newlines = newline_indices.len();
    let num_bytes = text.as_bytes().len();

    if newline_indices.len() == 0 {
        return Point::new(point.row, point.column + num_bytes);
    } else {
        let last_newline_index = newline_indices.last().unwrap();
        return Point::new(
            point.row + num_newlines,
            num_bytes - last_newline_index.0 - 1,
        );
    }
}

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
    pub position_encoding: PositionEncodingKind,

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
        let ast = parser.parse(contents.as_str(), None).unwrap();
        let parse = aether_parser::parse(&contents, Default::default());
        let line_index = biome_line_index::LineIndex::new(&contents);

        Self {
            contents,
            version,
            ast,
            parse,
            line_index,
            position_encoding: PositionEncodingKind::UTF16,
            config: Default::default(),
        }
    }

    pub fn on_did_change(&mut self, parser: &mut Parser, params: &DidChangeTextDocumentParams) {
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

        for event in &params.content_changes {
            if let Err(err) = self.update(parser, event) {
                panic!("Failed to update document: {err:?}");
            }
        }

        // Set new version
        self.version = Some(new_version);
    }

    fn update(
        &mut self,
        parser: &mut Parser,
        change: &TextDocumentContentChangeEvent,
    ) -> Result<()> {
        // Extract edit range. Return without doing anything if there wasn't any actual edit.
        let range = match change.range {
            Some(r) => r,
            None => return Ok(()),
        };

        // Update the AST. We do this before updating the underlying document
        // contents, because edit computations need to be done using the current
        // state of the document (prior to the edit being applied) so that byte
        // offsets can be computed correctly.
        let ast = &mut self.ast;

        let tree_sitter::Range {
            start_byte,
            end_byte: old_end_byte,
            start_point,
            end_point: old_end_point,
        } = convert_lsp_range_to_tree_sitter_range(&self.contents, &self.line_index, range);

        let new_end_point = compute_point(start_point, &change.text);
        let new_end_byte = start_byte + change.text.as_bytes().len();

        // Confusing tree sitter names, the `start_position` is really a `Point`
        let edit = InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position: start_point,
            old_end_position: old_end_point,
            new_end_position: new_end_point,
        };

        ast.edit(&edit);

        // We can now re-parse incrementally by providing the old edited AST
        let ast = parser.parse(self.contents.as_str(), Some(&self.ast));
        self.ast = ast.unwrap();

        // Update the Rowan tree
        self.parse = aether_parser::parse_tree_sitter(&self.contents, &self.ast);

        // Now update the text
        self.contents
            .replace_range(start_byte..old_end_byte, &change.text);
        self.line_index = biome_line_index::LineIndex::new(&self.contents);

        Ok(())
    }

    pub fn get_line(&self, line: usize) -> Option<&str> {
        let line_start = *self.line_index.newlines.get(line)?;
        let line_end = self
            .line_index
            .newlines
            .get(line + 1)
            .copied()
            // if `line` is last, extract text until end of buffer
            .unwrap_or_else(|| (self.contents.len() as u32).into());

        let line_start_byte: usize = line_start.into();
        let line_end_byte: usize = line_end.into();

        self.contents.get(line_start_byte..line_end_byte)
    }

    /// Accessor that returns an annotated `RSyntaxNode` type.
    /// More convenient than the generic `biome_rowan::SyntaxNode<L>` type.
    pub fn syntax(&self) -> aether_syntax::RSyntaxNode {
        self.parse.syntax()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_computation() {
        // empty strings shouldn't do anything
        let point = compute_point(Point::new(0, 0), "");
        assert_eq!(point, Point::new(0, 0));

        let point = compute_point(Point::new(42, 42), "");
        assert_eq!(point, Point::new(42, 42));

        // text insertion without newlines should just extend the column position
        let point = compute_point(Point::new(0, 0), "abcdef");
        assert_eq!(point, Point::new(0, 6));

        // text insertion with newlines should change the row
        let point = compute_point(Point::new(0, 0), "abc\ndef\nghi");
        assert_eq!(point, Point::new(2, 3));

        let point = compute_point(Point::new(0, 0), "abcdefghi\n");
        assert_eq!(point, Point::new(1, 0));
    }

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
}
