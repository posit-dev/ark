//
// pipe.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use harp::error::Error;
use harp::eval::r_parse_eval;
use harp::eval::RParseEvalOptions;
use harp::object::RObject;
use stdext::local;
use stdext::IntoOption;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;

use crate::lsp::completions::sources::utils::completions_from_object_names;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::BinaryOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

#[derive(Clone)]
pub(super) struct PipeRoot {
    pub(super) name: String,

    /// If `None`, we found a pipe root and tried to evaluate it, but the
    /// condition was too complex
    pub(super) object: Option<RObject>,
}

pub(super) fn completions_from_pipe(root: Option<PipeRoot>) -> Result<Option<Vec<CompletionItem>>> {
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

/// Loop should be kept in sync with `completions_from_call()` so they find
/// the same call to detect the pipe root of
pub(super) fn find_pipe_root(context: &DocumentContext) -> Option<PipeRoot> {
    log::info!("find_pipe_root()");

    let mut node = context.node;
    let mut has_call = false;

    loop {
        if node.is_call() {
            // We look for pipe roots from here
            has_call = true;
            break;
        }

        // If we reach a brace list, bail
        if node.is_braced_expression() {
            break;
        }

        // Update the node
        node = match node.parent() {
            Some(node) => node,
            None => break,
        };
    }

    if !has_call {
        return None;
    }

    let name = find_pipe_root_name(context, &node);

    let object = match &name {
        Some(name) => eval_pipe_root(name),
        None => None,
    };

    name.map(|name| PipeRoot { name, object })
}

fn eval_pipe_root(name: &str) -> Option<RObject> {
    let options = RParseEvalOptions {
        forbid_function_calls: true,
        ..Default::default()
    };

    let value = r_parse_eval(name, options);

    // If we get an `UnsafeEvaluationError` here from setting
    // `forbid_function_calls`, we don't want that to prevent
    // other sources from contributing completions
    let value = match value {
        Ok(value) => value,
        Err(err) => match err {
            Error::UnsafeEvaluationError(_) => return None,
            _ => {
                log::error!("{err:?}");
                return None;
            },
        },
    };

    Some(value)
}

fn find_pipe_root_name(context: &DocumentContext, node: &Node) -> Option<String> {
    // Try to figure out the code associated with the 'root' of the pipe expression.
    let root = local! {

        let root = find_pipe_root_node(context, *node)?;
        is_pipe_operator(context, &root).into_option()?;

        // Get the left-hand side of the pipe expression.
        let mut lhs = root.child_by_field_name("lhs")?;
        while is_pipe_operator(context, &lhs) {
            lhs = lhs.child_by_field_name("lhs")?;
        }

        // Try to evaluate the left-hand side
        let root = context.document.contents.node_slice(&lhs).ok()?.to_string();
        Some(root)

    };

    root.map(|x| x.to_string())
}

fn find_pipe_root_node<'a>(context: &DocumentContext, mut node: Node<'a>) -> Option<Node<'a>> {
    let mut root = None;

    loop {
        if is_pipe_operator(context, &node) {
            root = Some(node);
        }

        node = match node.parent() {
            Some(node) => node,
            None => return root,
        }
    }
}

fn is_pipe_operator(context: &DocumentContext, node: &Node) -> bool {
    let node_type = node.node_type();

    if node_type == NodeType::BinaryOperator(BinaryOperatorType::Pipe) {
        // Native pipe
        return true;
    }

    if node_type == NodeType::BinaryOperator(BinaryOperatorType::Special) {
        // magrittr pipe
        let Some(node) = node.child_by_field_name("operator") else {
            return false;
        };

        match context.document.contents.node_slice(&node) {
            Ok(slice) => return slice == "%>%",
            Err(err) => {
                log::error!("{err:?}");
                return false;
            },
        }
    }

    return false;
}

#[cfg(test)]
mod tests {
    use harp::eval::r_parse_eval;
    use harp::eval::RParseEvalOptions;
    use tree_sitter::Point;

    use crate::lsp::completions::sources::composite::pipe::find_pipe_root;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::test::r_test;

    #[test]
    fn test_find_pipe_root_works_with_native_and_magrittr() {
        r_test(|| {
            // Place cursor between `()` of `bar()`
            let point = Point { row: 0, column: 19 };
            let document = Document::new("x |> foo() %>% bar()", None);
            let context = DocumentContext::new(&document, point, None);

            let root = find_pipe_root(&context).unwrap();
            assert_eq!(root.name, "x".to_string());
            assert!(root.object.is_none());
        });

        r_test(|| {
            // `%||%` is not a pipe!
            // Place cursor between `()` of `bar()`
            let point = Point { row: 0, column: 20 };
            let document = Document::new("x |> foo() %||% bar()", None);
            let context = DocumentContext::new(&document, point, None);

            let root = find_pipe_root(&context);
            assert!(root.is_none());
        });
    }

    #[test]
    fn test_find_pipe_root_finds_objects() {
        r_test(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            // Place cursor between `()`
            let point = Point { row: 0, column: 10 };
            let document = Document::new("x %>% foo()", None);
            let context = DocumentContext::new(&document, point, None);

            let root = find_pipe_root(&context).unwrap();
            assert_eq!(root.name, "x".to_string());
            assert!(root.object.is_none());

            // Set up a real `x` and try again
            r_parse_eval("x <- data.frame(a = 1)", options.clone()).unwrap();

            let root = find_pipe_root(&context).unwrap();
            assert_eq!(root.name, "x".to_string());
            assert!(root.object.is_some());

            // Clean up
            r_parse_eval("x <- NULL", options.clone()).unwrap();
        });
    }
}
