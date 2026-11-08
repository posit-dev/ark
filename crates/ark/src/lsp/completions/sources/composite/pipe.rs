//
// pipe.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use tree_sitter::Node;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::NodeTypeExt;

/// Text of the root of the pipe containing the cursor's call, as in `x` for
/// `x |> foo() %>% bar(|)`, where `|` marks the cursor.
pub(super) fn find_pipe_root(context: &CompletionContext) -> Option<String> {
    log::trace!("find_pipe_root()");

    let call_node = context.containing_call_node()?;

    match find_pipe_root_name(context.document_context, &call_node) {
        Ok(root) => root,
        Err(err) => {
            log::trace!("Error trying to find pipe root: {err}");
            None
        },
    }
}

fn find_pipe_root_name(context: &DocumentContext, node: &Node) -> anyhow::Result<Option<String>> {
    // Try to figure out the code associated with the 'root' of the pipe expression
    let Some(root) = find_pipe_root_node(context, *node)? else {
        return Ok(None);
    };
    if !root.is_pipe_operator(context.contents)? {
        return Ok(None);
    }

    // Get the left-hand side of the pipe expression
    let Some(mut lhs) = root.child_by_field_name("lhs") else {
        return Ok(None);
    };

    while lhs.is_pipe_operator(context.contents)? {
        lhs = match lhs.child_by_field_name("lhs") {
            Some(lhs) => lhs,
            None => return Ok(None),
        };
    }

    let root = lhs.node_as_str(context.contents)?.to_string();

    Ok(Some(root))
}

fn find_pipe_root_node<'a>(
    context: &DocumentContext,
    mut node: Node<'a>,
) -> anyhow::Result<Option<Node<'a>>> {
    let mut root = None;

    loop {
        if node.is_pipe_operator(context.contents)? {
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
    use crate::lsp::completions::tests::utils::request_at_cursor;

    #[test]
    fn test_find_pipe_root_works_with_native_and_magrittr() {
        let request = request_at_cursor("x |> foo() %>% bar(@)");
        assert_eq!(request.composite.pipe_root, Some(String::from("x")));

        // `%||%` is not a pipe.
        let request = request_at_cursor("x |> foo() %||% bar(@)");
        assert_eq!(request.composite.pipe_root, None);
    }

    #[test]
    fn test_no_pipe_root_outside_call() {
        let request = request_at_cursor("x |> fo@");
        assert_eq!(request.composite.pipe_root, None);
    }
}
