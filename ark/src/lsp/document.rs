// 
// document.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use ropey::Rope;
use tower_lsp::lsp_types::{TextDocumentContentChangeEvent};
use tree_sitter::{Parser, Point, Tree, InputEdit};

use crate::lsp::{logger::log_push, macros::unwrap, position::PositionExt, rope::RopeExt};

fn compute_position(point: &Point, text: &str) -> Point {
 
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
            point.row + num_newlines - 1,
            num_bytes - last_newline_index.0,
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

        // extract range (nothing to do if we don't have one)
        let range = match change.range {
            Some(r) => r,
            None => return,
        };

        // convert completion position [row, column] to character-based offsets
        let lhs = self.contents.line_to_char(range.start.line as usize) + range.start.character as usize;
        let rhs = self.contents.line_to_char(range.end.line as usize) + range.end.character as usize;

        // remove the old slice of text, and insert the new text
        self.contents.remove(lhs..rhs);
        self.contents.insert(lhs, change.text.as_str());

        // TODO: update the AST based on these edits. see examples:
        //
        // https://github.com/tree-sitter/tree-sitter/blob/b729029a403046375a77a8e320211da64da00629/cli/src/tests/parser_test.rs#L499-L535
        // https://github.com/tree-sitter/tree-sitter/blob/b729029a403046375a77a8e320211da64da00629/cli/src/parse.rs#L276-L294
        //
        // as I understand it, the process for editing a tree is:
        //
        // 1. call 'tree.edit(...)', with an input range describing what portion of the document was editted,
        // 2. call 'parser.parse(&source, &edit)' to re-parse the document
        //
        // the "hard" part is computing the new position offsets, based on the edited range + inserted text,
        // especially when that text contains newlines. for now, we'll just reparse the whole document and
        // that'll probably be fast enough.
        //
        // maybe it's just me, but the tree-sitter InputEdit struct is odd. here are my attempts to explain
        // the members. really, it's about replacing a range with another range, but tree-sitter does that
        // by describing how the size of a range 'changes' from some start byte. so, if you tried to replace
        //
        //    abc_def
        //
        // with
        //
        //    abc!!!def
        //
        // you would describe this edit having started at byte offset 3,
        // with 'old_end_byte' == 4 (because we removed 1 byte), and with
        // 'new_end_byte' == 6 (because we inserted 3 characters)
        let ast = unwrap!(&mut self.ast, {
            log_push!("Document.update(): no AST available");
            return;
        });

        // TODO: do we need to convert characters to bytes here
        let start_byte = self.contents.position_to_byte(range.start);
        let start_position = range.start.as_point();

        let old_end_byte = self.contents.position_to_byte(range.end);
        let new_end_byte = old_end_byte - start_byte + change.text.as_bytes().len();

        let old_end_position = range.end.as_point();
        let new_end_position = compute_position(&start_position, &change.text);

        let edit = InputEdit {
            start_byte, old_end_byte, new_end_byte,
            start_position, old_end_position, new_end_position
        };

        ast.edit(&edit);

        // we've edited the ast, now we can re-parse the document
        self.ast = self.parser.parse(self.contents.to_string(), Some(&ast));

    }


}
