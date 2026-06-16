//
// document_context.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use aether_lsp_utils::proto::to_proto;
use aether_lsp_utils::proto::PositionEncoding;
use tower_lsp::lsp_types;
use tree_sitter::Node;
use tree_sitter::Point;

#[cfg(test)]
use crate::lsp::ark_file::test_ark_file;
#[cfg(test)]
use crate::lsp::ark_file::ArkFile;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

#[derive(Debug)]
pub struct DocumentContext<'a> {
    /// We store extracted components of `&ArkFile` + `&dyn ArkDb` here because
    /// the latter can't be sent over an `r_task()`.
    pub tree: &'a tree_sitter::Tree,
    pub contents: &'a str,
    pub line_index: &'a biome_line_index::LineIndex,
    pub encoding: PositionEncoding,
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
    pub fn new(
        tree: &'a tree_sitter::Tree,
        contents: &'a str,
        line_index: &'a biome_line_index::LineIndex,
        encoding: PositionEncoding,
        point: Point,
        trigger: Option<String>,
    ) -> Self {
        let Some(node) = tree.root_node().find_smallest_spanning_node(point) else {
            panic!(
                "Failed to find spanning node containing point: {point} with contents '{contents}'"
            );
        };

        // find closest node at point
        let Some(closest_node) = tree.root_node().find_closest_node_to_point(point) else {
            // TODO: We really want to track this down and figure out what's happening
            // and fix it in `find_closest_node_to_point()`.
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
            tree,
            contents,
            line_index,
            encoding,
            node,
            closest_node,
            point,
            trigger,
        }
    }

    pub fn lsp_position_from_tree_sitter_point(
        &self,
        point: tree_sitter::Point,
    ) -> anyhow::Result<lsp_types::Position> {
        let line_col = biome_line_index::LineCol {
            line: point.row as u32,
            col: point.column as u32,
        };
        to_proto::position_from_line_col(line_col, self.line_index, self.encoding)
    }

    pub fn lsp_range_from_tree_sitter_range(
        &self,
        range: tree_sitter::Range,
    ) -> anyhow::Result<lsp_types::Range> {
        let start = self.lsp_position_from_tree_sitter_point(range.start_point)?;
        let end = self.lsp_position_from_tree_sitter_point(range.end_point)?;
        Ok(lsp_types::Range::new(start, end))
    }
}

/// Owns a `db` + `ArkFile` so unit tests can build a `DocumentContext` the same
/// way handlers do, borrowing the cached tree and line index from the database.
#[cfg(test)]
pub(crate) struct TestDocument {
    db: oak_db::OakDatabase,
    file: ArkFile,
}

#[cfg(test)]
impl TestDocument {
    pub(crate) fn new(contents: &str) -> Self {
        let (db, file) = test_ark_file(contents);
        Self { db, file }
    }

    pub(crate) fn context(&self, point: Point) -> DocumentContext<'_> {
        self.context_with_trigger(point, None)
    }

    pub(crate) fn context_with_trigger(
        &self,
        point: Point,
        trigger: Option<String>,
    ) -> DocumentContext<'_> {
        DocumentContext::new(
            self.file.tree_sitter(&self.db),
            self.file.contents(&self.db),
            self.file.line_index(&self.db),
            self.file.encoding,
            point,
            trigger,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::point_from_cursor;
    use crate::treesitter::NodeType;
    use crate::treesitter::NodeTypeExt;

    #[test]
    fn test_document_context_start_of_document() {
        // Empty document
        let (text, point) = point_from_cursor("@");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(context.node.node_as_str(context.contents).unwrap(), "");

        // Start of document with text
        let (text, point) = point_from_cursor("@1 + 1");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(context.node.node_as_str(context.contents).unwrap(), "1");
    }

    #[test]
    fn test_document_context_end_of_identifier() {
        // Cursor at end of identifier "lib" at position (0, 3)
        // This reproduced a panic where find_smallest_spanning_node returned None
        let (text, point) = point_from_cursor("lib@");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        // The node should be the identifier "lib"
        assert_eq!(context.node.node_as_str(context.contents).unwrap(), "lib");
    }

    #[test]
    fn test_document_context_cursor_on_empty_line() {
        // as if we're about to type on the second line
        let (text, point) = point_from_cursor("toupper(letters)\n@");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);

        assert_eq!(context.node.node_type(), NodeType::Program);
        assert_eq!(
            context.node.node_as_str(context.contents).unwrap(),
            "toupper(letters)\n"
        );

        assert_eq!(
            context.closest_node.node_type(),
            NodeType::Anonymous(String::from(")"))
        );
        assert_eq!(
            context.closest_node.node_as_str(context.contents).unwrap(),
            ")"
        );
    }
}
