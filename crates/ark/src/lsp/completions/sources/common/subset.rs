//
// subset.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::traits::point::PointExt;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

pub(crate) fn is_within_subset_delimiters(
    x: &Point,
    subset_node: &Node,
    subset_type: &NodeType,
) -> bool {
    let (open, close) = match subset_type {
        NodeType::Subset => ("[", "]"),
        NodeType::Subset2 => ("[[", "]]"),
        _ => std::unreachable!(),
    };

    let Some(arguments) = subset_node.child_by_field_name("arguments") else {
        return false;
    };

    let n_children = arguments.child_count();

    if n_children < 2 {
        return false;
    }

    let Some(open_node) = arguments.child(1 - 1) else {
        return false;
    };
    let Some(close_node) = arguments.child(n_children - 1) else {
        return false;
    };

    // Ensure open and closing nodes are the right type
    if !matches!(
        open_node.node_type(),
        NodeType::Anonymous(kind) if kind == open
    ) {
        return false;
    }
    if !matches!(
        close_node.node_type(),
        NodeType::Anonymous(kind) if kind == close
    ) {
        return false;
    }

    let contains_start = x.is_after_or_equal(open_node.end_position());
    let contains_end = x.is_before_or_equal(close_node.start_position());

    contains_start && contains_end
}
