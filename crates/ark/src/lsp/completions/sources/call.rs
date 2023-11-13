//
// call.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::bail;
use anyhow::Result;
use harp::error::Error;
use harp::eval::r_parse_eval;
use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use stdext::join;
use stdext::local;
use stdext::IntoOption;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;
use tree_sitter::Node;

use crate::lsp::completions::completion_item::completion_item_from_data_variable;
use crate::lsp::completions::completion_item::completion_item_from_parameter;
use crate::lsp::completions::sources::names::completions_from_object_names;
use crate::lsp::completions::sources::utils::set_sort_text_by_first_appearance;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::indexer;
use crate::lsp::traits::node::NodeExt;

pub fn completions_from_call(context: &DocumentContext) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_call()");

    let mut node = context.node;

    let mut has_possible_completions = false;
    let mut has_call_completions = false;

    let mut completions = vec![];

    loop {
        // Check for 'subset' completions.
        // `$` and `@` are handled elsewhere as they can't be mixed with
        // other call completions.
        if matches!(node.kind(), "[" | "[[") {
            has_possible_completions = true;

            const ENQUOTE: bool = true;

            if let Some(child) = node.child(0) {
                let text = child.utf8_text(context.source.as_bytes())?;
                completions.append(&mut completions_from_object_names(&text, ENQUOTE)?);
            }
        }

        // If we landed on a 'call', then we should provide parameter completions
        // for the associated callee if possible.
        if !has_call_completions && node.kind() == "call" {
            has_possible_completions = true;
            has_call_completions = true;

            let pipe_root_name = find_pipe_root_name(context, &node);
            let pipe_root_object = match &pipe_root_name {
                Some(text) => Some(eval_pipe_root(text)?),
                None => None,
            };

            if pipe_root_name.is_some() {
                let pipe_root_name = pipe_root_name.unwrap();
                let pipe_root_object = pipe_root_object.clone().unwrap();

                // Add in names of the `pipe_root_object` as possible completions
                if let Some(mut pipe_completions) =
                    completions_from_pipe_root(pipe_root_name.as_str(), pipe_root_object)?
                {
                    completions.append(&mut pipe_completions);
                }
            }

            // Check for standard call completions.
            if let Some(mut call_completions) =
                completions_from_standard_call(context, &node, pipe_root_object)?
            {
                completions.append(&mut call_completions);
            }
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

    if !has_possible_completions {
        // Didn't detect anything worth completing in this context,
        // let other sources add their own candidates instead
        return Ok(None);
    }

    // Prioritize argument names over variables.
    // These are currently the only two types of fields this source generates.
    for item in &mut completions {
        // Start with existing `sort_text` if one exists
        let text = match &item.sort_text {
            Some(sort_text) => sort_text,
            None => &item.label,
        };

        match item.kind {
            Some(CompletionItemKind::FIELD) => {
                item.sort_text = Some(join!["1-", text]);
            },
            Some(CompletionItemKind::VARIABLE) => {
                item.sort_text = Some(join!["2-", text]);
            },
            _ => {
                unreachable!();
            },
        }
    }

    Ok(Some(completions))
}

fn completions_from_pipe_root(root: &str, object: RObject) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_pipe_root()");

    let mut completions = vec![];

    unsafe {
        // Try to retrieve names from the resulting item
        let names = RFunction::new("base", "names")
            .add(object)
            .call()?
            .to::<Vec<String>>()?;

        for name in names {
            let item = completion_item_from_data_variable(&name, root, false)?;
            completions.push(item);
        }
    }

    Ok(Some(completions))
}

fn eval_pipe_root(root: &str) -> Result<RObject> {
    unsafe {
        let value = r_parse_eval(root, RParseEvalOptions {
            forbid_function_calls: true,
        });

        // If we get an `UnsafeEvaluationError` here from setting
        // `forbid_function_calls`, we don't want that to prevent
        // other sources from contributing completions
        let value = match value {
            Ok(value) => value,
            Err(err) => match err {
                Error::UnsafeEvaluationError(_) => return Ok(RObject::null()),
                _ => bail!("{err:?}"),
            },
        };

        Ok(value)
    }
}

fn find_pipe_root_name(context: &DocumentContext, node: &Node) -> Option<String> {
    // Try to figure out the code associated with the 'root' of the pipe expression.
    let root = local! {

        let root = find_pipe_root_node(*node)?;
        is_pipe_operator(&root).into_option()?;

        // Get the left-hand side of the pipe expression.
        let mut lhs = root.child_by_field_name("lhs")?;
        while is_pipe_operator(&lhs) {
            lhs = lhs.child_by_field_name("lhs")?;
        }

        // Try to evaluate the left-hand side
        let root = lhs.utf8_text(context.source.as_bytes()).ok()?;
        Some(root)

    };

    root.map(|x| x.to_string())
}

fn find_pipe_root_node(mut node: Node) -> Option<Node> {
    let mut root = None;

    loop {
        if is_pipe_operator(&node) {
            root = Some(node);
        }

        node = match node.parent() {
            Some(node) => node,
            None => return root,
        }
    }
}

fn is_pipe_operator(node: &Node) -> bool {
    matches!(node.kind(), "%>%" | "|>")
}

fn completions_from_standard_call(
    context: &DocumentContext,
    node: &Node,
    pipe_root_object: Option<RObject>,
) -> Result<Option<Vec<CompletionItem>>> {
    let marker = context
        .node
        .bwd_leaf_iter()
        .find_map(|node| match node.kind() {
            "(" | "comma" => Some("name"),
            "=" => Some("value"),
            "call" => Some("value"),
            _ => None,
        });

    // Get the caller text.
    let Some(callee) = node.child(0) else {
        return Ok(None);
    };

    let callee = callee.utf8_text(context.source.as_bytes())?;

    // - Prefer `pipe_root` as the first argument if it exists
    // - Then fall back to looking it up, if possible
    // - Otherwise use `NULL` to signal that we can't figure it out
    let object = match pipe_root_object {
        Some(object) => object,
        None => match get_first_argument(context, node)? {
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
    };

    // Try to evaluate the first argument
    let value = unsafe { r_parse_eval(text, options) };

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
