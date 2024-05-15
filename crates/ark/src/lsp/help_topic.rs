//
// help_topic.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use serde::Deserialize;
use serde::Serialize;
use tower_lsp::lsp_types::Position;
use tower_lsp::lsp_types::VersionedTextDocumentIdentifier;
use tree_sitter::Node;
use tree_sitter::Point;
use tree_sitter::Tree;

use crate::lsp;
use crate::lsp::documents::Document;
use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

pub static POSITRON_HELP_TOPIC_REQUEST: &'static str = "positron/textDocument/helpTopic";

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelpTopicParams {
    /// The document to provide a help topic for.
    pub text_document: VersionedTextDocumentIdentifier,
    /// The location of the cursor.
    pub position: Position,
}

#[derive(Debug, Eq, PartialEq, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HelpTopicResponse {
    /// The help topic appropriate for the cursor position.
    pub topic: String,
}

pub(crate) fn help_topic(
    tree: &tree_sitter::Tree,
    point: Point,
    document: &Document,
) -> anyhow::Result<Option<HelpTopicResponse>> {
    let Some(node) = locate_help_node(tree, point) else {
        lsp::log_warn!("help_topic(): No help node at position {point}");
        return Ok(None);
    };

    // Get the text of the node
    let text = document.contents.node_slice(&node)?.to_string();

    // Form the response
    let response = HelpTopicResponse { topic: text };

    lsp::log_info!(
        "help_topic(): Using help topic '{}' at position {}",
        response.topic,
        point
    );

    Ok(Some(response))
}

fn locate_help_node(tree: &Tree, point: Point) -> Option<Node> {
    let root = tree.root_node();

    let Some(mut node) = root.find_closest_node_to_point(point) else {
        return None;
    };

    // Find the nearest node that is an identifier.
    while !node.is_identifier() {
        if let Some(sibling) = node.prev_sibling() {
            // Move to an adjacent sibling if we can.
            node = sibling;
        } else if let Some(parent) = node.parent() {
            // If no sibling, check the parent.
            node = parent;
        } else {
            return None;
        }
    }

    // Check if this identifier is part of a namespace operator. If it is, we send
    // back the whole `pkg::fun` text, regardless of which side the user was on.
    // Even if they are at `p<>kg::fun`, we assume they really want docs for `fun`.
    let node = match node.parent() {
        Some(parent) if matches!(parent.node_type(), NodeType::NamespaceOperator(_)) => parent,
        Some(_) => node,
        None => node,
    };

    Some(node)
}

#[cfg(test)]
mod tests {
    use tree_sitter::Parser;

    use crate::lsp::help_topic::locate_help_node;
    use crate::test::point_from_cursor;

    #[test]
    fn test_locate_help_node() {
        let language = tree_sitter_r::language();

        let mut parser = Parser::new();
        parser
            .set_language(&language)
            .expect("failed to create parser");

        // On the RHS
        let (text, point) = point_from_cursor("dplyr::ac@ross(x:y, sum)");
        let tree = parser.parse(text.as_str(), None).unwrap();
        let node = locate_help_node(&tree, point).unwrap();
        let text = node.utf8_text(text.as_bytes()).unwrap();
        assert_eq!(text, "dplyr::across");

        // On the LHS (Returns function help for `across()`, not package help for `dplyr`,
        // as we assume that is more useful for the user).
        let (text, point) = point_from_cursor("dpl@yr::across(x:y, sum)");
        let tree = parser.parse(text.as_str(), None).unwrap();
        let node = locate_help_node(&tree, point).unwrap();
        let text = node.utf8_text(text.as_bytes()).unwrap();
        assert_eq!(text, "dplyr::across");

        // In the operator
        let (text, point) = point_from_cursor("dplyr:@:across(x:y, sum)");
        let tree = parser.parse(text.as_str(), None).unwrap();
        let node = locate_help_node(&tree, point).unwrap();
        let text = node.utf8_text(text.as_bytes()).unwrap();
        assert_eq!(text, "dplyr::across");

        // Internal `:::`
        let (text, point) = point_from_cursor("dplyr:::ac@ross(x:y, sum)");
        let tree = parser.parse(text.as_str(), None).unwrap();
        let node = locate_help_node(&tree, point).unwrap();
        let text = node.utf8_text(text.as_bytes()).unwrap();
        assert_eq!(text, "dplyr:::across");
    }
}
