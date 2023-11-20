//
// call.rs
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
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;

use super::pipe::PipeRoot;
use crate::lsp::completions::completion_item::completion_item_from_parameter;
use crate::lsp::completions::sources::utils::call_node_position_type;
use crate::lsp::completions::sources::utils::set_sort_text_by_first_appearance;
use crate::lsp::completions::sources::utils::CallNodePositionType;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::indexer;

pub(super) fn completions_from_call(
    context: &DocumentContext,
    root: Option<PipeRoot>,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_call()");

    let mut node = context.node;
    let mut has_call = false;

    loop {
        // If we landed on a 'call', then we should provide parameter completions
        // for the associated callee if possible.
        if node.kind() == "call" {
            has_call = true;
            break;
        }

        // If we reach a brace list, bail.
        if node.kind() == "{" {
            break;
        }

        // Update the node.
        node = match node.parent() {
            Some(node) => node,
            None => break,
        };
    }

    if !has_call {
        // Didn't detect anything worth completing in this context,
        // let other sources add their own candidates instead
        return Ok(None);
    }

    // Now that we know we are in a call, detect if we are in a location where
    // we should provide argument completions, i.e. if we are in the `name`
    // position of:
    //
    // fn(name = value)
    //    ~~~~
    //
    if call_node_position_type(&context.node, context.point) != CallNodePositionType::Name {
        return Ok(None);
    }

    // Get the caller text.
    let Some(callee) = node.child(0) else {
        return Ok(None);
    };

    let callee = callee.utf8_text(context.source.as_bytes())?;

    // - Prefer `root` as the first argument if it exists
    // - Then fall back to looking it up, if possible
    // - Otherwise use `NULL` to signal that we can't figure it out
    let object = match root {
        Some(root) => match root.object {
            Some(object) => object,
            None => RObject::null(),
        },
        None => match get_first_argument(context, &node)? {
            Some(object) => object,
            None => RObject::null(),
        },
    };

    let completions = completions_from_arguments(context, &callee, object)?;

    Ok(Some(completions))
}

fn get_first_argument(context: &DocumentContext, node: &Node) -> Result<Option<RObject>> {
    // Get the first argument, if any (object used for dispatch).
    // TODO: We should have some way of matching calls, so we can
    // take a function signature from R and see how the call matches
    // to that object.

    let Some(arguments) = node.child_by_field_name("arguments") else {
        return Ok(None);
    };

    let mut cursor = arguments.walk();
    let mut children = arguments.children_by_field_name("argument", &mut cursor);

    let Some(argument) = children.next() else {
        return Ok(None);
    };

    // Don't want first argument to be named
    let None = argument.child_by_field_name("name") else {
        return Ok(None);
    };

    let Some(value) = argument.child_by_field_name("value") else {
        return Ok(None);
    };

    let text = value.utf8_text(context.source.as_bytes())?;

    let options = RParseEvalOptions {
        forbid_function_calls: true,
        ..Default::default()
    };

    // Try to evaluate the first argument
    let value = r_parse_eval(text, options);

    // If the user is writing pseudocode, this object might not exist yet,
    // in which case we just want to ignore the error from trying to evaluate it
    // and just provide typical completions.
    // If we get an `UnsafeEvaluationError` here from setting
    // `forbid_function_calls`, we don't even log that one, as that is
    // expected to happen with complex first inputs that call functions.
    let value = match value {
        Ok(value) => value,
        Err(err) => match err {
            Error::UnsafeEvaluationError(_) => return Ok(None),
            _ => {
                log::info!("get_first_argument(): Failed to evaluate first argument: {err}");
                return Ok(None);
            },
        },
    };

    Ok(Some(value))
}

fn completions_from_arguments(
    context: &DocumentContext,
    callable: &str,
    object: RObject,
) -> Result<Vec<CompletionItem>> {
    log::info!("completions_from_arguments({callable:?})");

    let mut completions = vec![];

    // Check for a function defined in the workspace that can provide parameters.
    if let Some((_path, entry)) = indexer::find(callable) {
        match entry.data {
            indexer::IndexEntryData::Function {
                ref name,
                ref arguments,
            } => {
                for argument in arguments {
                    match completion_item_from_parameter(argument, name, &context.point) {
                        Ok(item) => completions.push(item),
                        Err(err) => log::error!("{err:?}"),
                    }
                }
            },
            indexer::IndexEntryData::Section { level: _, title: _ } => {
                // nothing to do
            },
        }
    }

    unsafe {
        // Otherwise, try to retrieve completion names from the object itself.
        let r_callable = r_parse_eval(callable, RParseEvalOptions {
            forbid_function_calls: true,
            ..Default::default()
        })?;

        let strings = RFunction::from(".ps.completions.formalNames")
            .add(r_callable)
            .add(object)
            .call()?
            .to::<Vec<String>>()?;

        // Return the names of these formals.
        for string in strings.iter() {
            match completion_item_from_parameter(string, callable, &context.point) {
                Ok(item) => completions.push(item),
                Err(err) => log::error!("{err:?}"),
            }
        }
    }

    // Only 1 call worth of arguments are added to the completion set.
    // We add a custom sort order to order them based on their position in the
    // underlying function.
    set_sort_text_by_first_appearance(&mut completions);

    Ok(completions)
}
