//
// pipe.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use harp::error::Error;
use harp::eval::RParseEvalOptions;
use harp::object::RObject;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;

use crate::console;
use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::sources::utils::completions_from_object_names;
use crate::lsp::completions::sources::CompletionSource;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::NodeTypeExt;

pub(super) struct PipeSource;

impl CompletionSource for PipeSource {
    fn name(&self) -> &'static str {
        "pipe"
    }

    fn provide_completions(
        &self,
        completion_context: &CompletionContext,
    ) -> anyhow::Result<Option<Vec<CompletionItem>>> {
        completions_from_pipe(completion_context.pipe_root())
    }
}

#[derive(Clone)]
pub struct PipeRoot {
    pub(super) name: String,

    /// If `None`, we found a pipe root and tried to evaluate it, but the
    /// condition was too complex
    pub(super) object: Option<RObject>,
}

fn completions_from_pipe(root: Option<PipeRoot>) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    let Some(root) = root else {
        // No pipe
        return Ok(None);
    };

    let name = root.name;

    let Some(object) = root.object else {
        // There was a pipe, but can't detect root object
        return Ok(None);
    };

    const ENQUOTE: bool = false;

    Ok(Some(completions_from_object_names(
        object,
        name.as_str(),
        ENQUOTE,
    )?))
}

pub fn find_pipe_root(
    context: &DocumentContext,
    call_node: Option<Node>,
) -> anyhow::Result<Option<PipeRoot>> {
    log::trace!("find_pipe_root()");

    let Some(call_node) = call_node else {
        return Ok(None);
    };

    let name = find_pipe_root_name(context, &call_node)?;

    let object = match &name {
        Some(name) => eval_pipe_root(name),
        None => None,
    };

    Ok(name.map(|name| PipeRoot { name, object }))
}

fn eval_pipe_root(name: &str) -> Option<RObject> {
    let options = RParseEvalOptions {
        forbid_function_calls: true,
        env: console::selected_env(),
    };

    let value = harp::parse_eval(name, options);

    // If we get an `UnsafeEvaluationError` here from setting
    // `forbid_function_calls`, we don't want that to prevent
    // other sources from contributing completions.
    // If we get a `TryCatchError`, that is typically an 'object not found' error resulting
    // from the user typing pseudocode. Log those at info level without a full backtrace.
    let value = match value {
        Ok(value) => value,
        Err(err) => match err {
            Error::UnsafeEvaluationError(_) => return None,
            Error::TryCatchError { message, .. } => {
                log::trace!("Can't evaluate pipe root: {message}");
                return None;
            },
            _ => {
                log::error!("Can't evaluate pipe root: {err:?}");
                return None;
            },
        },
    };

    Some(value)
}

fn find_pipe_root_name(context: &DocumentContext, node: &Node) -> anyhow::Result<Option<String>> {
    // Try to figure out the code associated with the 'root' of the pipe expression
    let Some(root) = find_pipe_root_node(context, *node)? else {
        return Ok(None);
    };
    if !root.is_pipe_operator(&context.document.contents)? {
        return Ok(None);
    }

    // Get the left-hand side of the pipe expression
    let Some(mut lhs) = root.child_by_field_name("lhs") else {
        return Ok(None);
    };

    while lhs.is_pipe_operator(&context.document.contents)? {
        lhs = match lhs.child_by_field_name("lhs") {
            Some(lhs) => lhs,
            None => return Ok(None),
        };
    }

    // Try to evaluate the left-hand side
    let root = lhs.node_as_str(&context.document.contents)?.to_string();

    Ok(Some(root))
}

fn find_pipe_root_node<'a>(
    context: &DocumentContext,
    mut node: Node<'a>,
) -> anyhow::Result<Option<Node<'a>>> {
    let mut root = None;

    loop {
        if node.is_pipe_operator(&context.document.contents)? {
            root = Some(node);
        }

        node = match node.parent() {
            Some(node) => node,
            None => return Ok(root),
        }
    }
}

#[cfg(test)]
mod tests {
    use harp::eval::RParseEvalOptions;

    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::sources::composite::pipe::find_pipe_root;
    use crate::lsp::document::Document;
    use crate::lsp::document_context::DocumentContext;
    use crate::r_task;
    use crate::treesitter::node_find_containing_call;

    #[test]
    fn test_find_pipe_root_works_with_native_and_magrittr() {
        r_task(|| {
            // Place cursor between `()` of `bar()`
            let (text, point) = point_from_cursor("x |> foo() %>% bar(@)");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let call_node = node_find_containing_call(context.node);

            let root = find_pipe_root(&context, call_node).unwrap().unwrap();
            assert_eq!(root.name, "x".to_string());
            assert!(root.object.is_none());
        });

        r_task(|| {
            // `%||%` is not a pipe!
            // Place cursor between `()` of `bar()`
            let (text, point) = point_from_cursor("x |> foo() %||% bar(@)");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let call_node = node_find_containing_call(context.node);

            let root = find_pipe_root(&context, call_node).unwrap();
            assert!(root.is_none());
        });
    }

    #[test]
    fn test_find_pipe_root_finds_objects() {
        r_task(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            // Place cursor between `()`
            let (text, point) = point_from_cursor("x %>% foo(@)");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let call_node = node_find_containing_call(context.node);

            let root = find_pipe_root(&context, call_node).unwrap().unwrap();
            assert_eq!(root.name, "x".to_string());
            assert!(root.object.is_none());

            // Set up a real `x` and try again
            harp::parse_eval("x <- data.frame(a = 1)", options.clone()).unwrap();

            let root = find_pipe_root(&context, call_node).unwrap().unwrap();
            assert_eq!(root.name, "x".to_string());
            assert!(root.object.is_some());

            // Clean up
            harp::parse_eval("remove(x)", options.clone()).unwrap();
        });
    }
}
