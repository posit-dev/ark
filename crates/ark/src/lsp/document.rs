//
// document.rs
//
// Copyright (C) 2022 by RStudio, PBC
//
//

use ropey::Rope;
use tower_lsp::lsp_types::TextDocumentContentChangeEvent;
use tree_sitter::InputEdit;
use tree_sitter::Parser;
use tree_sitter::Point;
use tree_sitter::Tree;

use crate::macros::unwrap;
use crate::lsp::logger::dlog;
use crate::lsp::traits::position::PositionExt;
use crate::lsp::traits::rope::RopeExt;

fn compute_point(point: Point, text: &str) -> Point {

    // figure out where the newlines in this edit are
    let newline_indices : Vec<_> = text.match_indices('\n').collect();
    let num_newlines = newline_indices.len();
    let num_bytes = text.as_bytes().len();

    if newline_indices.len() == 0 {
        return Point::new(
            point.row,
            point.column + num_bytes,
        );
    } else {
        let last_newline_index = newline_indices.last().unwrap();
        return Point::new(
            point.row + num_newlines,
            num_bytes - last_newline_index.0 - 1
        );
    }
}


pub(crate) struct Document {

    // The document's textual contents.
    pub contents: Rope,

    // The parser used to generate the AST.
    pub parser: Parser,

    // The document's AST.
    pub ast: Option<Tree>,

}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document").field("contents", &self.contents).field("ast", &self.ast).finish()
    }
}

impl Document {

    pub fn new(contents: &str) -> Self {

        // create initial document from rope
        let document = Rope::from(contents);

        // create a parser for this document
        let mut parser = Parser::new();
        parser.set_language(tree_sitter_r::language()).expect("failed to create parser");
        let ast = parser.parse(contents, None);

        // return generated document
        Self { contents: document, parser, ast }
    }

    pub fn update(&mut self, change: &TextDocumentContentChangeEvent) {

        // Extract edit range. Nothing to do if there wasn't an edit.
        let range = match change.range {
            Some(r) => r,
            None => return,
        };

        // Update the AST. We do this before updating the underlying document
        // contents, because edit computations need to be done using the current
        // state of the document (prior to the edit being applied) so that byte
        // offsets can be computed correctly.
        let ast = unwrap!(self.ast.as_mut(), {
            dlog!("no AST available");
            return;
        });

        let start_byte = self.contents.position_to_byte(range.start);
        let start_position = range.start.as_point();

        let old_end_byte = self.contents.position_to_byte(range.end);
        let new_end_byte = start_byte + change.text.as_bytes().len();

        let old_end_position = range.end.as_point();
        let new_end_position = compute_point(start_position, &change.text);

        let edit = InputEdit {
            start_byte, old_end_byte, new_end_byte,
            start_position, old_end_position, new_end_position
        };

        ast.edit(&edit);

        // Now, apply edits to the underlying document.
        let lhs = self.contents.line_to_char(range.start.line as usize) + range.start.character as usize;
        let rhs = self.contents.line_to_char(range.end.line as usize) + range.end.character as usize;

        // Remove the old slice of text, and insert the new slice of text.
        self.contents.remove(lhs..rhs);
        self.contents.insert(lhs, change.text.as_str());

        // We've edited the AST, and updated the document. We can now re-parse.
        self.ast = self.parser.parse(self.contents.to_string(), Some(&ast));

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
}
