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
    /// Formerly known just as "node". This renaming unblocks completion
    /// improvements, where we really do want to focus on the smallest node
    /// that **actually contains** the point. Future cleanup elsewhere in the
    /// language server might allow us to standardize on just "node" again,
    /// although I suspect hover might always require a notion of closest node.
    pub closest_node: Node<'a>,
    pub point: Point,
    pub trigger: Option<String>,
}

impl<'a> DocumentContext<'a> {
    pub fn new(document: &'a Document, point: Point, trigger: Option<String>) -> Self {
        // get reference to AST
        let ast = &document.ast;

        let Some(node) = ast.root_node().find_smallest_spanning_node(point) else {
            let contents = document.contents.to_string();
            panic!(
                "Failed to find spanning node containing point: {point} with contents '{contents}'"
            );
        };

        // find closest node at point
        let Some(closest_node) = ast.root_node().find_closest_node_to_point(point) else {
            // TODO: We really want to track this down and figure out what's happening
            // and fix it in `find_closest_node_to_point()`.
            let contents = document.contents.to_string();
            panic!("Failed to find closest node to point: {point} with contents '{contents}'");
        };

        // build document context
        DocumentContext {
            document,
            node,
            closest_node,
            point,
            trigger,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::traits::rope::RopeExt;
    use crate::treesitter::NodeType;
    use crate::treesitter::NodeTypeExt;

    #[test]
    fn test_document_context_start_of_document() {
        let point = Point { row: 0, column: 0 };

        // Empty document
        let document = Document::new("", None);
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
        let document = Document::new("1 + 1", None);
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

    #[test]
    fn test_document_context_cursor_on_empty_line() {
        let document = Document::new("toupper(letters)\n", None);
        let point = Point { row: 1, column: 0 }; // as if we're about to type on the second line
        let context = DocumentContext::new(&document, point, None);

        assert_eq!(context.node.node_type(), NodeType::Program);
        assert_eq!(
            context
                .document
                .contents
                .node_slice(&context.node)
                .unwrap()
                .to_string(),
            "toupper(letters)\n"
        );

        assert_eq!(
            context.closest_node.node_type(),
            NodeType::Anonymous(String::from(")"))
        );
        assert_eq!(
            context
                .document
                .contents
                .node_slice(&context.closest_node)
                .unwrap()
                .to_string(),
            ")"
        );
    }
}
