//
// call.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

use harp::error::Error;
use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::utils::r_is_function;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::completion_item::completion_item_from_parameter;
use crate::lsp::completions::sources::utils::call_node_position_type;
use crate::lsp::completions::sources::utils::set_sort_text_by_first_appearance;
use crate::lsp::completions::sources::utils::CallNodePositionType;
use crate::lsp::completions::sources::CompletionSource;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::indexer;
use crate::lsp::traits::rope::RopeExt;

pub(super) struct CallSource;

impl CompletionSource for CallSource {
    fn name(&self) -> &'static str {
        "call"
    }

    fn provide_completions(
        &self,
        completion_context: &CompletionContext,
    ) -> anyhow::Result<Option<Vec<CompletionItem>>> {
        completions_from_call(completion_context)
    }
}

fn completions_from_call(
    context: &CompletionContext,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    let Some(node) = context.containing_call_node() else {
        // Not in a call, let other sources add their own candidates
        return Ok(None);
    };

    let document_context = context.document_context;
    let point = document_context.point;

    // Now that we know we are in a call, detect if we are in a location where
    // we should provide argument completions, i.e. if we are in the `name`
    // position of:
    //
    // fn(name = value)
    //    ~~~~
    //
    match call_node_position_type(&document_context.node, point) {
        // We should provide argument completions. Ambiguous states like
        // `fn(arg<tab>)` or `fn(x, arg<tab>)` should still get argument
        // completions.
        CallNodePositionType::Name => (),
        CallNodePositionType::Ambiguous => (),
        // We shouldn't provide argument completions, let another source
        // contribute completions
        CallNodePositionType::Value |
        CallNodePositionType::Outside |
        CallNodePositionType::Unknown => return Ok(None),
    };

    // Get the caller text.
    let Some(callee) = node.child(0) else {
        return Ok(None);
    };

    let callee = document_context
        .document
        .contents
        .node_slice(&callee)?
        .to_string();

    // - Prefer `root` as the first argument if it exists
    // - Then fall back to looking it up, if possible
    // - Otherwise use `NULL` to signal that we can't figure it out
    let root = context.pipe_root();
    let object = match root {
        Some(root) => match root.object {
            Some(object) => object,
            None => RObject::null(),
        },
        None => match get_first_argument(document_context, &node)? {
            Some(object) => object,
            None => RObject::null(),
        },
    };

    completions_from_arguments(document_context, &callee, object)
}

fn get_first_argument(context: &DocumentContext, node: &Node) -> anyhow::Result<Option<RObject>> {
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

    let text = context.document.contents.node_slice(&value)?.to_string();

    let options = RParseEvalOptions {
        forbid_function_calls: true,
        ..Default::default()
    };

    // Try to evaluate the first argument
    let value = harp::parse_eval(text.as_str(), options);

    // If we get an `UnsafeEvaluationError` here from setting
    // `forbid_function_calls`, we don't even log that one, as that is
    // expected to happen with complex first inputs that call functions.
    // If we get a `TryCatchError`, that is typically an 'object not found' error resulting
    // from the user typing pseudocode. Log those at info level without a full backtrace.
    let value = match value {
        Ok(value) => value,
        Err(err) => match err {
            Error::UnsafeEvaluationError(_) => return Ok(None),
            Error::TryCatchError { message, .. } => {
                log::trace!("Can't evaluate first argument: {message}");
                return Ok(None);
            },
            _ => {
                log::error!("Can't evaluate first argument: {err}");
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
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    log::trace!("completions_from_arguments({callable:?})");

    // Try looking up session function first, as the "current state of the world"
    // will provide the most accurate completions
    if let Some(completions) = completions_from_session_arguments(context, callable, object)? {
        return Ok(Some(completions));
    }

    if let Some(completions) = completions_from_workspace_arguments(context, callable)? {
        return Ok(Some(completions));
    }

    Ok(None)
}

fn completions_from_session_arguments(
    context: &DocumentContext,
    callable: &str,
    object: RObject,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    log::trace!("completions_from_session_arguments({callable:?})");

    let mut completions = vec![];

    // Try to retrieve completion names from the object itself.
    // If we can find it, this is the most accurate way to provide completions,
    // as it represents the current state of the world and adds completions
    // for S3 methods based on `object`.
    let r_callable = harp::parse_eval(callable, RParseEvalOptions {
        forbid_function_calls: true,
        ..Default::default()
    });

    let r_callable = match r_callable {
        Ok(r_callable) => r_callable,
        Err(err) => match err {
            // LHS of the call was too complex to evaluate.
            harp::error::Error::UnsafeEvaluationError(_) => return Ok(None),
            // LHS of the call evaluated to an error. Totally possible if the
            // user is writing pseudocode or if they haven't loaded the
            // package they are working on. Don't want to propagate an error here.
            _ => return Ok(None),
        },
    };

    if !r_is_function(r_callable.sexp) {
        // Found the `callable` but it isn't a function in the current state
        // of the world, return an empty completion set.
        return Ok(Some(completions));
    }

    let strings = unsafe {
        RFunction::from(".ps.completions.formalNames")
            .add(r_callable)
            .add(object)
            .call()?
            .to::<Vec<String>>()?
    };

    // Return the names of these formals.
    for string in strings.iter() {
        match completion_item_from_parameter(string, callable, context) {
            Ok(item) => completions.push(item),
            Err(err) => log::error!("{err:?}"),
        }
    }

    // Only 1 call worth of arguments are added to the completion set.
    // We add a custom sort order to order them based on their position in the
    // underlying function.
    set_sort_text_by_first_appearance(&mut completions);

    Ok(Some(completions))
}

fn completions_from_workspace_arguments(
    context: &DocumentContext,
    callable: &str,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    log::trace!("completions_from_workspace_arguments({callable:?})");

    // Try to find the `callable` in the workspace and use its arguments
    // if we can
    let Some((_path, entry)) = indexer::find(callable) else {
        // Didn't find any workspace object with this name
        return Ok(None);
    };

    let mut completions = vec![];

    match entry.data {
        indexer::IndexEntryData::Function { name, arguments } => {
            for argument in arguments {
                match completion_item_from_parameter(argument.as_str(), name.as_str(), context) {
                    Ok(item) => completions.push(item),
                    Err(err) => log::error!("{err:?}"),
                }
            }
        },
        indexer::IndexEntryData::Section { level: _, title: _ } => {
            // Not a function
            return Ok(None);
        },
        indexer::IndexEntryData::Variable { .. } => return Ok(None),
    }

    // Only 1 call worth of arguments are added to the completion set.
    // We add a custom sort order to order them based on their position in the
    // underlying function.
    set_sort_text_by_first_appearance(&mut completions);

    Ok(Some(completions))
}

#[cfg(test)]
mod tests {
    use harp::eval::RParseEvalOptions;

    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::completion_context::CompletionContext;
    use crate::lsp::completions::sources::composite::call::completions_from_call;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::lsp::state::WorldState;
    use crate::r_task;

    #[test]
    fn test_completions_after_user_types_part_of_an_argument_name() {
        r_task(|| {
            // Right after `tab`
            let (text, point) = point_from_cursor("match(tab@)");
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let state = WorldState::default();
            let context = CompletionContext::new(&document_context, &state);
            let completions = completions_from_call(&context).unwrap().unwrap();

            // We detect this as a `name` position and return all possible completions
            assert_eq!(completions.len(), 4);
            assert_eq!(completions.get(0).unwrap().label, "x = ");
            assert_eq!(completions.get(1).unwrap().label, "table = ");

            // Right after `tab`
            let (text, point) = point_from_cursor("match(1, tab@)");
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let state = WorldState::default();
            let context = CompletionContext::new(&document_context, &state);
            let completions = completions_from_call(&context).unwrap().unwrap();

            // We detect this as a `name` position and return all possible completions
            // (TODO: Should not return `x` as a possible completion)
            assert_eq!(completions.len(), 4);
            assert_eq!(completions.get(0).unwrap().label, "x = ");
            assert_eq!(completions.get(1).unwrap().label, "table = ");
        })
    }

    #[test]
    fn test_session_arguments() {
        // Can't find the function
        r_task(|| {
            // Place cursor between `()`
            let (text, point) = point_from_cursor("not_a_known_function(@)");
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let state = WorldState::default();
            let context = CompletionContext::new(&document_context, &state);
            let completions = completions_from_call(&context).unwrap();
            assert!(completions.is_none());
        });

        // Basic session argument lookup
        r_task(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            // Set up a function with arguments in the session
            harp::parse_eval("my_fun <- function(y, x) x + y", options.clone()).unwrap();

            // Place cursor between `()`
            let (text, point) = point_from_cursor("my_fun(@)");
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let state = WorldState::default();
            let context = CompletionContext::new(&document_context, &state);
            let completions = completions_from_call(&context).unwrap().unwrap();

            assert_eq!(completions.len(), 2);

            // Retains positional ordering
            let completion = completions.get(0).unwrap();
            assert_eq!(completion.label, "y = ");

            let completion = completions.get(1).unwrap();
            assert_eq!(completion.label, "x = ");

            // Place just before the `()`
            let (text, point) = point_from_cursor("my_fun@()");
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let state = WorldState::default();
            let context = CompletionContext::new(&document_context, &state);
            let completions = completions_from_call(&context).unwrap();
            assert!(completions.is_none());

            // Place just after the `()`
            let (text, point) = point_from_cursor("my_fun()@");
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let state = WorldState::default();
            let context = CompletionContext::new(&document_context, &state);
            let completions = completions_from_call(&context).unwrap();
            assert!(completions.is_none());

            // Clean up
            harp::parse_eval("remove(my_fun)", options.clone()).unwrap();
        });

        // Case where the session object isn't a function
        r_task(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            // Set up an object in the session
            harp::parse_eval("my_fun <- 1", options.clone()).unwrap();

            // Place cursor between `()`
            let (text, point) = point_from_cursor("my_fun(@)");
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let state = WorldState::default();
            let context = CompletionContext::new(&document_context, &state);
            let completions = completions_from_call(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 0);

            // Clean up
            harp::parse_eval("remove(my_fun)", options.clone()).unwrap();
        })
    }

    #[test]
    fn test_completions_multiline_call() {
        r_task(|| {
            // No arguments typed yet
            let (text, point) = point_from_cursor("match(\n  @\n)");
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let state = WorldState::default();
            let context = CompletionContext::new(&document_context, &state);
            let completions = completions_from_call(&context).unwrap().unwrap();

            assert_eq!(completions.len(), 4);
            assert_eq!(completions.get(0).unwrap().label, "x = ");
            assert_eq!(completions.get(1).unwrap().label, "table = ");

            // Partially typed argument
            let (text, point) = point_from_cursor("match(\n  tab@\n)");
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let state = WorldState::default();
            let context = CompletionContext::new(&document_context, &state);
            let completions = completions_from_call(&context).unwrap().unwrap();

            assert_eq!(completions.len(), 4);
            assert_eq!(completions.get(0).unwrap().label, "x = ");
            assert_eq!(completions.get(1).unwrap().label, "table = ");

            // Partially typed second argument
            let (text, point) = point_from_cursor("match(\n  1,\n  tab@\n)");
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let state = WorldState::default();
            let context = CompletionContext::new(&document_context, &state);
            let completions = completions_from_call(&context).unwrap().unwrap();

            assert_eq!(completions.len(), 4);
            assert_eq!(completions.get(0).unwrap().label, "x = ");
            assert_eq!(completions.get(1).unwrap().label, "table = ");
        })
    }

    #[test]
    fn test_completions_in_value_position() {
        r_task(|| {
            fn assert_no_call_completions(code_with_cursor: &str) {
                let (text, point) = point_from_cursor(code_with_cursor);
                let document = Document::new(text.as_str(), None);
                let document_context = DocumentContext::new(&document, point, None);
                let state = WorldState::default();
                let context = CompletionContext::new(&document_context, &state);
                let completions = completions_from_call(&context).unwrap();

                // We shouldn't get completions in value position
                assert!(completions.is_none());
            }

            // Single line, with space
            assert_no_call_completions("match(x = @)");

            // Single line, no space
            assert_no_call_completions("match(x =@)");

            // Multiline case, with space
            assert_no_call_completions("match(\n  x = @\n)");

            // Multiline case, no space
            assert_no_call_completions("match(\n  x =@\n)");
        });
    }
}
