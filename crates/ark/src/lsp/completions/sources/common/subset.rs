//
// subset.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::traits::point::PointExt;

pub(crate) fn is_within_subset_delimiters(x: &Point, subset_node: &Node) -> bool {
    let Some(arguments) = subset_node.child_by_field_name("arguments") else {
        return false;
    };

    let Some(open) = arguments.child_by_field_name("open") else {
        return false;
    };
    let Some(close) = arguments.child_by_field_name("close") else {
        return false;
    };

    let contains_start = x.is_after_or_equal(open.end_position());
    let contains_end = x.is_before_or_equal(close.start_position());

    contains_start && contains_end
}
