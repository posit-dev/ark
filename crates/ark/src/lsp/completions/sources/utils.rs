//
// utils.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use harp::error::Error;
use harp::eval::r_parse_eval;
use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use regex::Regex;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::completions::completion_item::completion_item_from_data_variable;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::point::PointExt;
use crate::lsp::traits::rope::RopeExt;

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
        .document
        .contents
        .node_slice(&context.node)
        .and_then(|x| Ok(x.to_string().starts_with(".")))
        .unwrap_or(false);

    if !user_requested_dot {
        completions.retain(|x| !x.label.starts_with("."));
    }
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
    match node.kind() {
        "(" => {
            if point.is_before_or_equal(node.start_position()) {
                // Before the `(`
                return CallNodePositionType::Outside;
            } else {
                // Must be a name position
                return CallNodePositionType::Name;
            }
        },
        ")" => {
            if point.is_after_or_equal(node.end_position()) {
                // After the `)`
                return CallNodePositionType::Outside;
            } else {
                // Let previous leaf determine type (i.e. did the `)`
                // follow a `=` or a `,`?)
                return call_prev_leaf_position_type(&node, false);
            }
        },
        "comma" => return CallNodePositionType::Name,
        "=" => return CallNodePositionType::Value,
        // Like `fn(arg<tab>)` or `fn(x = 1, arg<tab>)` (which are ambiguous)
        // or `fn(x = arg<tab>)` (which is clearly a `Value`)
        "identifier" => return call_prev_leaf_position_type(&node, true),
        _ => {
            // Probably a complex node inside `()`. Typically a `Value`
            // unless we are at the very beginning of the node.

            // For things like `vctrs::vec_sort(x = 1, |2)` where you typed
            // the argument value but want to go back and fill in the name.
            if point == node.start_position() {
                return call_prev_leaf_position_type(&node, false);
            }

            return CallNodePositionType::Value;
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

    match previous.kind() {
        "(" | "comma" => {
            if allow_ambiguous {
                // i.e. `fn(arg<tab>)` or `fn(x, arg<tab>)` where it can be
                // ambiguous whether we are on a `Name` or a `Value`.
                return CallNodePositionType::Ambiguous;
            } else {
                return CallNodePositionType::Name;
            }
        },
        "=" => return CallNodePositionType::Value,
        _ => return CallNodePositionType::Value,
    }
}

pub(super) fn completions_from_evaluated_object_names(
    name: &str,
    enquote: bool,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_evaluated_object_names({name:?})");

    let options = RParseEvalOptions {
        forbid_function_calls: true,
        ..Default::default()
    };

    // Try to evaluate the object
    let object = r_parse_eval(name, options);

    // If the user is writing pseudocode, this object might not exist yet,
    // in which case we just want to ignore the error from trying to evaluate it
    // and just provide typical completions.
    // If we get an `UnsafeEvaluationError` here from setting
    // `forbid_function_calls`, we don't even log that one, as that is
    // expected to happen with complex inputs.
    let object = match object {
        Ok(object) => object,
        Err(err) => match err {
            Error::UnsafeEvaluationError(_) => return Ok(None),
            _ => {
                log::info!(
                    "completions_from_evaluated_object_names(): Failed to evaluate first argument: {err}"
                );
                return Ok(None);
            },
        },
    };

    Ok(Some(completions_from_object_names(object, name, enquote)?))
}

pub(super) fn completions_from_object_names(
    object: RObject,
    name: &str,
    enquote: bool,
) -> Result<Vec<CompletionItem>> {
    log::info!("completions_from_object_names({object:?})");

    let mut completions = vec![];

    unsafe {
        let variable_names = RFunction::new("base", "names")
            .add(object)
            .call()?
            .to::<Vec<String>>()?;

        for variable_name in variable_names {
            match completion_item_from_data_variable(&variable_name, name, enquote) {
                Ok(item) => completions.push(item),
                Err(err) => log::error!("{err:?}"),
            }
        }
    }

    Ok(completions)
}

#[cfg(test)]
mod tests {
    use tree_sitter::Point;

    use crate::lsp::completions::sources::utils::call_node_position_type;
    use crate::lsp::completions::sources::utils::CallNodePositionType;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;

    #[test]
    fn test_call_node_position_type() {
        // Before `(`, but on it
        let point = Point { row: 0, column: 3 };
        let document = Document::new("fn ()");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(context.node.kind(), "(");
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Outside
        );

        // After `)`, but on it
        let point = Point { row: 0, column: 4 };
        let document = Document::new("fn()");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(context.node.kind(), ")");
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Outside
        );

        // After `(`, but on it
        let point = Point { row: 0, column: 3 };
        let document = Document::new("fn()");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(context.node.kind(), "(");
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Name
        );

        // After `x`
        let point = Point { row: 0, column: 4 };
        let document = Document::new("fn(x)");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Ambiguous
        );

        // After `x`
        let point = Point { row: 0, column: 7 };
        let document = Document::new("fn(1, x)");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Ambiguous
        );

        // Directly after `,`
        let point = Point { row: 0, column: 5 };
        let document = Document::new("fn(x, )");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(context.node.kind(), "comma");
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Name
        );

        // After `,`, but on `)`
        let point = Point { row: 0, column: 6 };
        let document = Document::new("fn(x, )");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(context.node.kind(), ")");
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Name
        );

        // After `=`
        let point = Point { row: 0, column: 6 };
        let document = Document::new("fn(x =)");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(context.node.kind(), "=");
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Value
        );

        // In an expression
        let point = Point { row: 0, column: 4 };
        let document = Document::new("fn(1 + 1)");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(context.node.kind(), "float");
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Value
        );

        let point = Point { row: 0, column: 8 };
        let document = Document::new("fn(1 + 1)");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(context.node.kind(), "float");
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Value
        );

        // Right before an expression
        // (special case where we still provide argument completions)
        let point = Point { row: 0, column: 6 };
        let document = Document::new("fn(1, 1 + 1)");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(context.node.kind(), "float");
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Name
        );

        // After an identifier, before the `)`, with whitespace between them,
        // but on the `)`
        let point = Point { row: 0, column: 5 };
        let document = Document::new("fn(x )");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(context.node.kind(), ")");
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Value
        );

        // After an identifier, before the `)`, with whitespace between them,
        // but on the identifier
        let point = Point { row: 0, column: 4 };
        let document = Document::new("fn(x )");
        let context = DocumentContext::new(&document, point, None);
        assert_eq!(context.node.kind(), "identifier");
        assert_eq!(
            call_node_position_type(&context.node, context.point),
            CallNodePositionType::Ambiguous
        );
    }
}
