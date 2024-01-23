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
    pub point: Point,
    pub trigger: Option<String>,
}

impl<'a> DocumentContext<'a> {
    pub fn new(document: &'a Document, point: Point, trigger: Option<String>) -> Self {
        // get reference to AST
        let ast = &document.ast;

        // find node at point
        let node = ast.root_node().find_closest_node_to_point(point).unwrap();

        // build document context
        DocumentContext {
            document,
            node,
            point,
            trigger,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::traits::rope::RopeExt;

    #[test]
    fn test_document_context_start_of_document() {
        let point = Point { row: 0, column: 0 };

        // Empty document
        let document = Document::new("");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(
            context
                .document
                .contents
                .node_slice(&context.node)
                .unwrap()
                .to_string(),
            "".to_string()
        );

        // Start of document with text
        let document = Document::new("1 + 1");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(
            context
                .document
                .contents
                .node_slice(&context.node)
                .unwrap()
                .to_string(),
            "1".to_string()
        );
    }
}
