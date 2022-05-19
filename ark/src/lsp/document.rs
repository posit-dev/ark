/*
 * document.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use ropey::Rope;
use tower_lsp::lsp_types::{TextDocumentContentChangeEvent, CompletionParams, CompletionItem};
use tree_sitter::{Parser, TreeCursor, Node, Point};

use crate::lsp::{cursor::TreeCursorExt, backend::Backend, logger::LOGGER};


#[derive(Debug)]
pub(crate) struct Document {

    // The document's textual contents, as a rope.
    document: Rope,

}

impl Document {

    pub fn new(contents: String) -> Self {
        Self { document: Rope::from(contents) }
    }

    pub fn update(&mut self, change: &TextDocumentContentChangeEvent) {

        let range = match change.range {
            Some(r) => r,
            None => return,
        };

        // convert completion position [row, column] to character-based offsets
        let lhs = self.document.line_to_char(range.start.line as usize) + range.start.character as usize;
        let rhs = self.document.line_to_char(range.end.line as usize) + range.end.character as usize;

        // remove the old slice of text, and insert the new text
        self.document.remove(lhs..rhs);
        self.document.insert(lhs, change.text.as_str());

    }

    pub fn append_completions(&mut self, params: &CompletionParams, completions: &mut Vec<CompletionItem>) {

        // TODO: can we incrementally update AST as edits come in?
        // Or should we defer building the AST until completions are requested?
        // The fact that the LSP methods are async seems to make this more challening,
        // since the parser and its nodes cannot survive across 'await' boundaries.

        // create the parser
        let mut parser = Parser::new();
        parser.set_language(tree_sitter_r::language()).expect("failed to create parser");

        // parse the document
        let contents = self.document.to_string();
        let ast = parser.parse(&contents, None).expect("failed to parse code");

        // get a cursor, and move it to the completion cursor location
        // TODO: from the documentation, it looks like CompletionParams will
        // give us a character offset; however, tree-sitter expects a byte offset
        // when moving a cursor. we'll need to convert the completion position
        // to a byte-oriented position when attempting to place the cursor
        let mut cursor = ast.walk();

        cursor.go_to_point(Point {
            row: params.text_document_position.position.line as usize,
            column: params.text_document_position.position.character as usize,
        });

        let message = format!("Node at point: {:?}", cursor.node());
        unsafe { LOGGER.append(message.as_str()) };

        cursor.find_parent(|node| {
            let message = format!("Node: {:?}", node);
            unsafe { LOGGER.append(message.as_str()) };
            return true;
        });

        // walk(&mut cursor, |node| {

        //     // check for assignments
        //     if node.kind() == "left_assignment" && node.child_count() > 0 {
        //         let lhs = node.child(0).unwrap();
        //         if lhs.kind() == "identifier" {
        //             let variable = lhs.utf8_text(contents.as_bytes());
        //             if let Ok(variable) = variable {
        //                 let detail = format!("Defined on row {}", node.range().start_point.row + 1);
        //                 completions.push(CompletionItem::new_simple(variable.to_string(), detail));
        //             }
        //         }
        //     }

        //     return true;

        // });

    }

}
