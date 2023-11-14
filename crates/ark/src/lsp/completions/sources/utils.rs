//
// utils.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use regex::Regex;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;

use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::node::NodeExt;

pub(super) fn set_sort_text_by_first_appearance(completions: &mut Vec<CompletionItem>) {
    let size = completions.len();

    // Surely there's a better way to figure out what factor of 10 the `size`
    // fits in, but I can't think of it right now
    let mut width = 1;
    let mut value = 10;

    while size >= value {
        value = value * 10;
        width += 1;
    }

    for (i, item) in completions.iter_mut().enumerate() {
        // Start with existing `sort_text` if one exists
        let text = match &item.sort_text {
            Some(sort_text) => sort_text,
            None => &item.label,
        };
        // Append an integer left padded with `0`s
        let prefix = format!("{:0width$}", i, width = width);
        let sort_text = format!("{prefix}-{text}");
        item.sort_text = Some(sort_text);
    }
}

pub(super) fn set_sort_text_by_words_first(completions: &mut Vec<CompletionItem>) {
    // `_` is considered a word character but we typically want those at the end so:
    // - First `^` for "starts with"
    // - Second `^` for "not the \W_"
    // - `\W_` for "non word characters plus `_`"
    // Result is "starts with any word character except `_`"
    let pattern = Regex::new(r"^[^\W_]").unwrap();

    for item in completions {
        // Start with existing `sort_text` if one exists
        let text = match &item.sort_text {
            Some(sort_text) => sort_text,
            None => &item.label,
        };

        if pattern.is_match(text) {
            item.sort_text = Some(format!("1-{text}"));
        } else {
            item.sort_text = Some(format!("2-{text}"));
        }
    }
}

pub(super) fn filter_out_dot_prefixes(
    context: &DocumentContext,
    completions: &mut Vec<CompletionItem>,
) {
    // Remove completions that start with `.` unless the user explicitly requested them
    let user_requested_dot = context
        .node
        .utf8_text(context.source.as_bytes())
        .and_then(|x| Ok(x.starts_with(".")))
        .unwrap_or(false);

    if !user_requested_dot {
        completions.retain(|x| !x.label.starts_with("."));
    }
}

#[derive(PartialEq)]
pub(super) enum NodeCallPositionType {
    Name,
    Value,
}

pub(super) fn node_call_position_type(node: &Node) -> NodeCallPositionType {
    // First try current node, before beginning to recurse
    match node.kind() {
        "(" | "comma" => return NodeCallPositionType::Name,
        "=" => return NodeCallPositionType::Value,
        "call" => return NodeCallPositionType::Value,
        _ => (),
    }

    // Now do a backwards leaf search, looking for a `(` or `,`.
    // If we hit a `=` that means we were in a `value` position instead
    // of a `name` position.
    node.bwd_leaf_iter()
        .find_map(|node| match node.kind() {
            "(" | "comma" => Some(NodeCallPositionType::Name),
            "=" => Some(NodeCallPositionType::Value),
            "call" => Some(NodeCallPositionType::Value),
            _ => None,
        })
        .unwrap_or(NodeCallPositionType::Value)
}
