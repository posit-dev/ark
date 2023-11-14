//
// document_context.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp::lsp_types::TextDocumentPositionParams;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::documents::Document;
use crate::lsp::traits::position::PositionExt;
use crate::lsp::traits::tree::TreeExt;

#[derive(Debug)]
pub struct DocumentContext<'a> {
    pub document: &'a Document,
    pub node: Node<'a>,
    pub source: String,
    pub point: Point,
}

impl<'a> DocumentContext<'a> {
    pub fn new(document: &'a Document, position: &TextDocumentPositionParams) -> Self {
        // convert to tree-sitter point
        let point = position.position.as_point();

        // get reference to AST
        let ast = &document.ast;

        // find node at point
        let node = ast.node_at_point(point);

        // convert document contents to a string once, to be reused elsewhere
        let source = document.contents.to_string();

        // build document context
        DocumentContext {
            document,
            node,
            source,
            point,
        }
    }
}
