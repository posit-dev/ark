//
// utils.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use regex::Regex;
use tower_lsp_server::ls_types::CompletionItem;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::point::PointExt;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

pub(crate) fn has_priority_prefix(sort_text: &str) -> bool {
    // If we want to force a completion item to the top of the list,
    // we pre-emptively set its `sort_text` to begin with "0-".
    // We usually don't want to mess with such `sort_text` downstream.
    // Not sure it will remain as simple as "0-", hence this helper.
    sort_text.starts_with("0-")
}

pub(crate) fn set_sort_text_by_first_appearance(completions: &mut [CompletionItem]) {
    let size = completions.len();

    // Surely there's a better way to figure out what factor of 10 the `size`
    // fits in, but I can't think of it right now
    let mut width = 1;
    let mut value = 10;

    while size >= value {
        value *= 10;
        width += 1;
    }

    for (i, item) in completions.iter_mut().enumerate() {
        // Start with existing `sort_text` if one exists
        let text = match &item.sort_text {
            Some(sort_text) => sort_text,
            None => &item.label,
        };
        // Append an integer left padded with `0`s.
        //
        // We don't check if pre-existing `sort_text` has any specific form,
        // because there's no known overlap between the use cases for sorting by
        // first appearance and wanting to hoist a specific completion item to
        // the top of the list.
        let prefix = format!("{:0width$}", i, width = width);
        let sort_text = format!("{prefix}-{text}");
        item.sort_text = Some(sort_text);
    }
}

pub(crate) fn set_sort_text_by_words_first(completions: &mut Vec<CompletionItem>) {
    // `_` is considered a word character but we typically want those at the end so:
    // - First `^` for "starts with"
    // - Second `^` for "not the \W_"
    // - `\W_` for "non word characters plus `_`"
    // Result is "starts with any word character except `_`"
    let pattern = Regex::new(r"^[^\W_]").unwrap();

    for item in completions {
        // Start with existing `sort_text` if one exists
        let text = match &item.sort_text {
            Some(sort_text) => {
                // If it looks like the `sort_text` already has a prefix meant
                // to bring this item to the top, don't prepend another prefix.
                if has_priority_prefix(sort_text) {
                    continue;
                }
                sort_text
            },
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
    if !user_requested_dot(context) {
        remove_dot_prefixes(completions);
    }
}

/// Whether the token under the cursor starts with `.`.
pub(crate) fn user_requested_dot(context: &DocumentContext) -> bool {
    context
        .node
        .node_as_str(context.contents)
        .map(|x| x.starts_with("."))
        .unwrap_or(false)
}

pub(crate) fn remove_dot_prefixes(completions: &mut Vec<CompletionItem>) {
    completions.retain(|x| !x.label.starts_with("."));
}

#[derive(PartialEq, Debug)]
pub(super) enum CallNodePositionType {
    Name,
    Value,
    Ambiguous,
    Outside,
    Unknown,
}

pub(super) fn call_node_position_type(node: &Node, point: Point) -> CallNodePositionType {
    match node.node_type() {
        NodeType::Arguments => CallNodePositionType::Name,
        NodeType::Anonymous(kind) if kind == "(" => {
            if point.is_before_or_equal(node.start_position()) {
                // Before the `(`
                CallNodePositionType::Outside
            } else {
                // Must be a name position
                CallNodePositionType::Name
            }
        },
        NodeType::Anonymous(kind) if kind == ")" => {
            if point.is_after_or_equal(node.end_position()) {
                // After the `)`
                CallNodePositionType::Outside
            } else {
                // Let previous leaf determine type (i.e. did the `)`
                // follow a `=` or a `,`?)
                call_prev_leaf_position_type(node, false)
            }
        },
        NodeType::Comma => CallNodePositionType::Name,
        NodeType::Anonymous(kind) if kind == "=" => CallNodePositionType::Value,
        // Like `fn(arg<tab>)` or `fn(x = 1, arg<tab>)` (which are ambiguous)
        // or `fn(x = arg<tab>)` (which is clearly a `Value`)
        NodeType::Identifier => call_prev_leaf_position_type(node, true),
        _ => {
            // Probably a complex node inside `()`. Typically a `Value`
            // unless we are at the very beginning of the node.

            // For things like `vctrs::vec_sort(x = 1, |2)` where you typed
            // the argument value but want to go back and fill in the name.
            if point == node.start_position() {
                return call_prev_leaf_position_type(node, false);
            }

            CallNodePositionType::Value
        },
    }
}

fn call_prev_leaf_position_type(node: &Node, allow_ambiguous: bool) -> CallNodePositionType {
    let Some(previous) = node.prev_leaf() else {
        // We expect a previous leaf to exist anywhere we use this, so if it
        // doesn't exist then we return this marker type that tells us we should
        // probably investigate our heuristics.
        log::warn!(
            "Expected `node` to have a previous leaf. Is `call_node_position_type()` written correctly?"
        );
        return CallNodePositionType::Unknown;
    };

    let after_open_parenthesis_or_comma = if allow_ambiguous {
        // i.e. `fn(arg<tab>)` or `fn(x, arg<tab>)` where it can be
        // ambiguous whether we are on a `Name` or a `Value`.
        CallNodePositionType::Ambiguous
    } else {
        CallNodePositionType::Name
    };

    match previous.node_type() {
        NodeType::Comma => after_open_parenthesis_or_comma,
        NodeType::Anonymous(kind) if kind == "(" => after_open_parenthesis_or_comma,
        NodeType::Anonymous(kind) if kind == "=" => CallNodePositionType::Value,
        _ => CallNodePositionType::Value,
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::sources::utils::call_node_position_type;
    use crate::lsp::completions::sources::utils::CallNodePositionType;
    use crate::lsp::document_context::TestDocument;
    use crate::treesitter::NodeType;
    use crate::treesitter::NodeTypeExt;

    #[test]
    fn test_call_node_position_type() {
        // Before `(`, but on it
        let (text, point) = point_from_cursor("fn @()");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(
            context.node.node_type(),
            NodeType::Anonymous(String::from("("))
        );
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Outside
        );

        // After `)`, but on it
        let (text, point) = point_from_cursor("fn()@");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(
            context.node.node_type(),
            NodeType::Anonymous(String::from(")"))
        );
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Outside
        );

        // After `(`, but on it
        let (text, point) = point_from_cursor("fn(@)");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(
            context.node.node_type(),
            NodeType::Anonymous(String::from("("))
        );
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Name
        );

        // After `x`
        let (text, point) = point_from_cursor("fn(x@)");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Ambiguous
        );

        // After `x`
        let (text, point) = point_from_cursor("fn(1, x@)");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Ambiguous
        );

        // Directly after `,`
        let (text, point) = point_from_cursor("fn(x,@ )");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(context.node.node_type(), NodeType::Comma);
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Name
        );

        // After `,`, but on `)`
        let (text, point) = point_from_cursor("fn(x, @)");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(
            context.node.node_type(),
            NodeType::Anonymous(String::from(")"))
        );
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Name
        );

        // After `=`
        let (text, point) = point_from_cursor("fn(x =@ )");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(
            context.node.node_type(),
            NodeType::Anonymous(String::from("="))
        );
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Value
        );

        // In an expression
        let (text, point) = point_from_cursor("fn(1@ + 1)");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(context.node.node_type(), NodeType::Float);
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Value
        );

        let (text, point) = point_from_cursor("fn(1 + 1@)");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(context.node.node_type(), NodeType::Float);
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Value
        );

        // Right before an expression
        // (special case where we still provide argument completions)
        let (text, point) = point_from_cursor("fn(1, @1 + 1)");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(context.node.node_type(), NodeType::Float);
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Name
        );

        // After an identifier, before the `)`, with whitespace between them,
        // but on the `)`
        let (text, point) = point_from_cursor("fn(x @)");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert_eq!(
            context.node.node_type(),
            NodeType::Anonymous(String::from(")"))
        );
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Value
        );

        // After an identifier, before the `)`, with whitespace between them,
        // but on the identifier
        let (text, point) = point_from_cursor("fn(x@ )");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        assert!(context.node.is_identifier());
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Ambiguous
        );

        // After `(`, and on own line
        let (text, point) = point_from_cursor("fn(\n  @\n)");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);

        assert_eq!(context.node.node_type(), NodeType::Arguments);
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Name
        );
    }
}
