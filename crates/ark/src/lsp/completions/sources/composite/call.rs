//
// call.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::Position;
use tree_sitter::Node;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::completion_item::completion_item_from_parameter;
use crate::lsp::completions::request::CallDispatch;
use crate::lsp::completions::request::CallRequest;
use crate::lsp::completions::sources::utils::call_node_position_type;
use crate::lsp::completions::sources::utils::set_sort_text_by_first_appearance;
use crate::lsp::completions::sources::utils::CallNodePositionType;
use crate::lsp::db::ArkDb;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::indexer;
use crate::lsp::open_file::lsp_position_from_tree_sitter_point;
use crate::lsp::traits::node::NodeExt;

/// Recognizes an argument name position in a call. With a pipe root, the
/// root takes the place of the first argument for S3 dispatch.
pub(super) fn recognize_call(
    context: &CompletionContext,
    has_pipe_root: bool,
) -> anyhow::Result<Option<CallRequest>> {
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

    let callee = callee.node_as_str(document_context.contents)?.to_string();

    let dispatch = if has_pipe_root {
        CallDispatch::PipeRoot
    } else {
        match get_first_argument(document_context, &node)? {
            Some(argument) => CallDispatch::FirstArgument(argument),
            None => CallDispatch::None,
        }
    };

    let position = lsp_position_from_tree_sitter_point(
        point,
        document_context.line_index,
        document_context.encoding,
    )?;

    Ok(Some(CallRequest {
        callee,
        dispatch,
        position,
    }))
}

fn get_first_argument(context: &DocumentContext, node: &Node) -> anyhow::Result<Option<String>> {
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

    Ok(Some(value.node_as_str(context.contents)?.to_string()))
}

/// Workspace formals for `callable`, used when the session declines the call slot.
pub(super) fn completions_from_workspace_arguments(
    db: &dyn ArkDb,
    callable: &str,
    position: Position,
) -> anyhow::Result<Vec<CompletionItem>> {
    log::trace!("completions_from_workspace_arguments({callable:?})");

    let mut completions = vec![];

    // Try to find the `callable` in the workspace and use its arguments
    // if we can
    let Some(entry) = indexer::find(db, callable) else {
        // Didn't find any workspace object with this name
        return Ok(completions);
    };

    match entry.data {
        indexer::IndexEntryData::Function { name, arguments } => {
            for argument in arguments {
                match completion_item_from_parameter(argument.as_str(), name.as_str(), position) {
                    Ok(item) => completions.push(item),
                    Err(err) => log::error!("{err:?}"),
                }
            }
        },
        indexer::IndexEntryData::Section { level: _, title: _ } => {
            // Not a function
            return Ok(completions);
        },
        indexer::IndexEntryData::Variable { .. } => return Ok(completions),
        indexer::IndexEntryData::Method { .. } => return Ok(completions),
    }

    // Only 1 call worth of arguments are added to the completion set.
    // We add a custom sort order to order them based on their position in the
    // underlying function.
    set_sort_text_by_first_appearance(&mut completions);

    Ok(completions)
}

#[cfg(test)]
mod tests {
    use crate::lsp::completions::finish;
    use crate::lsp::completions::request::CallDispatch;
    use crate::lsp::completions::request::CallRequest;
    use crate::lsp::completions::request::RCompletionReplies;
    use crate::lsp::completions::tests::utils::request_at_cursor;
    use crate::lsp::completions::tests::utils::static_completions_with_workspace;

    fn call_request_at_cursor(code_with_cursor: &str) -> Option<CallRequest> {
        request_at_cursor(code_with_cursor).composite.call
    }

    #[test]
    fn test_call_request() {
        let request = call_request_at_cursor("match(1, tab@)").unwrap();
        assert_eq!(request.callee, "match");
        assert_eq!(
            request.dispatch,
            CallDispatch::FirstArgument(String::from("1"))
        );

        // The pipe root supplies S3 dispatch even with an explicit first argument.
        let request = call_request_at_cursor("x |> match(1, tab@)").unwrap();
        assert_eq!(request.dispatch, CallDispatch::PipeRoot);

        // Named first arguments are not used for dispatch.
        let request = call_request_at_cursor("match(x = 1, tab@)").unwrap();
        assert_eq!(request.dispatch, CallDispatch::None);
    }

    #[test]
    fn test_no_call_request_outside_parentheses() {
        assert_eq!(call_request_at_cursor("my_fun@()"), None);

        assert_eq!(call_request_at_cursor("my_fun()@"), None);
    }

    #[test]
    fn test_no_call_request_in_value_position() {
        assert_eq!(call_request_at_cursor("match(x = @)"), None);

        assert_eq!(call_request_at_cursor("match(x =@)"), None);

        assert_eq!(call_request_at_cursor("match(\n  x = @\n)"), None);

        assert_eq!(call_request_at_cursor("match(\n  x =@\n)"), None);
    }

    #[test]
    fn test_workspace_arguments_without_session() {
        let (completions, _) =
            static_completions_with_workspace("my_fun(@)", "my_fun <- function(y, x) x + y");
        let completions = finish(completions, RCompletionReplies::default());

        let labels: Vec<&str> = completions.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, vec!["y = ", "x = "]);
    }
}
