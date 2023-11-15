//
// document_context.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::documents::Document;
use crate::lsp::traits::node::NodeExt;

#[derive(Debug)]
pub struct DocumentContext<'a> {
    pub document: &'a Document,
    pub node: Node<'a>,
    pub source: String,
    pub point: Point,
}

impl<'a> DocumentContext<'a> {
    pub fn new(document: &'a Document, point: Point) -> Self {
        // get reference to AST
        let ast = &document.ast;

        // find node at point
        let node = ast.root_node().find_closest_node_to_point(point).unwrap();

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
