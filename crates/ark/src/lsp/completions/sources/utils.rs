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
pub(super) enum CallNodePositionType {
    Name,
    Value,
    Other,
}

pub(super) fn call_node_position_type(node: &Node, point: Point) -> CallNodePositionType {
    match node.kind() {
        "(" => {
            if point.is_before_or_equal(node.start_position()) {
                // Before the `(`
                return CallNodePositionType::Other;
            } else {
                // Must be a name position
                return CallNodePositionType::Name;
            }
        },
        ")" => {
            if point.is_after_or_equal(node.end_position()) {
                // After the `)`
                return CallNodePositionType::Other;
            } else {
                // Let previous leaf determine type (i.e. did the `)`
                // follow a `=` or a `,`?)
                match node.prev_leaf() {
                    Some(node) => return call_node_position_type(&node, point),
                    None => return CallNodePositionType::Other,
                }
            }
        },
        "comma" => return CallNodePositionType::Name,
        "=" => return CallNodePositionType::Value,
        _ => {
            if point == node.start_position() {
                // For things like `vctrs::vec_sort(x = 1, |2)` where you typed
                // the argument value but want to go back and fill in the name.
                match node.prev_leaf() {
                    Some(node) => return call_node_position_type(&node, point),
                    None => return CallNodePositionType::Other,
                }
            } else {
                return CallNodePositionType::Other;
            }
        },
    }
}

pub(super) fn completions_from_evaluated_object_names(
    name: &str,
    enquote: bool,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_evaluated_object_names({name:?})");

    let options = RParseEvalOptions {
        forbid_function_calls: true,
    };

    // Try to evaluate the object
    let object = unsafe { r_parse_eval(name, options) };

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
