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
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

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

        // Fix up node selection in an edge case that arises from how cursor
        // position interacts with node span semantics.
        //
        // Tree-sitter node coordinates refer to position BETWEEN characters.
        // Node spans are inclusive on the left and exclusive on the right, in
        // terms of whether a cursor is considered to be inside the node.
        //
        //       0  1  2  3  4  5  6  7  8
        //       ┌──┬──┬──┬──┬──┬──┬──┬──┐
        //   0   │ o│ p│ t│ i│ o│ n│ s│ (│
        //       └──┴──┴──┴──┴──┴──┴──┴──┘
        //
        //       0  1  2  3  4  5  6    program [0, 0] - [3, 0]
        //       ┌──┬──┬──┬──┬──┬──┐      call [0, 0] - [2, 1]
        //   1   │  │  │ a│  │ =│  │        function: identifier [0, 0] - [0, 7]
        //       └──┴──┴──┴──┴──┴──┘        arguments: arguments [0, 7] - [2, 1]
        //                                    open: ( [0, 7] - [0, 8]
        //       0  1                         argument: argument [1, 2] - [1, 5]
        //       ┌──┐                           name: identifier [1, 2] - [1, 3]
        //   2   │ )│                           = [1, 4] - [1, 5]
        //       └──┘                         close: ) [2, 0] - [2, 1]
        //
        // Imagine the cursor is at [1, 6], i.e. the end of the second line.
        // The smallest spanning node is, counterintuitively, the 'Arguments'
        // node.
        // It is more favorable for completions to start in (or in a child of)
        // the 'Argument' node (with text "a = ").
        // In this case, `closest_node` is the anonymous "=" node and is a
        // better candidate for completions.
        let node = if node.node_type() == NodeType::Arguments &&
            closest_node.node_type() == NodeType::Anonymous(String::from("="))
        {
            closest_node
        } else {
            node
        };

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
    use crate::treesitter::node_text;
    use crate::treesitter::NodeType;
    use crate::treesitter::NodeTypeExt;

    #[test]
    fn test_document_context_start_of_document() {
        let point = Point { row: 0, column: 0 };

        // Empty document
        let document = Document::new("", None);
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(
            node_text(&context.node, &context.document.contents).unwrap(),
            ""
        );

        // Start of document with text
        let document = Document::new("1 + 1", None);
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(
            node_text(&context.node, &context.document.contents).unwrap(),
            "1"
        );
    }

    #[test]
    fn test_document_context_cursor_on_empty_line() {
        let document = Document::new("toupper(letters)\n", None);
        let point = Point { row: 1, column: 0 }; // as if we're about to type on the second line
        let context = DocumentContext::new(&document, point, None);

        assert_eq!(context.node.node_type(), NodeType::Program);
        assert_eq!(
            node_text(&context.node, &context.document.contents).unwrap(),
            "toupper(letters)\n"
        );

        assert_eq!(
            context.closest_node.node_type(),
            NodeType::Anonymous(String::from(")"))
        );
        assert_eq!(
            node_text(&context.closest_node, &context.document.contents).unwrap(),
            ")"
        );
    }
}
