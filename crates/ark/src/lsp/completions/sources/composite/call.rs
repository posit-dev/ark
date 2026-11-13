//
// call.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use legacy_ide::coordinates::lsp_position_from_tree_sitter_point;
use legacy_ide::traits::node::NodeExt;
use tower_lsp_server::ls_types::CompletionItem;
use tree_sitter::Node;

use crate::lsp::call_arguments::describe_callee;
use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::completion_item::completion_item_from_parameter;
use crate::lsp::completions::request::CallDispatch;
use crate::lsp::completions::request::CallRequest;
use crate::lsp::completions::sources::utils::call_node_position_type;
use crate::lsp::completions::sources::utils::set_sort_text_by_first_appearance;
use crate::lsp::completions::sources::utils::CallNodePositionType;
use crate::lsp::document_context::DocumentContext;

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

/// Formals from the static description of the callee recognized by
/// [recognize_call()]. These are the call slot's items when the session
/// declines it, for instance while R is busy or when the callee is only
/// defined in a file the session hasn't run.
pub(super) fn completions_from_callee_description(
    context: &CompletionContext,
    request: &CallRequest,
) -> anyhow::Result<Vec<CompletionItem>> {
    log::trace!("completions_from_callee_description({:?})", request.callee);

    let mut completions = vec![];

    let Some(callee) = context
        .containing_call_node()
        .and_then(|node| node.child(0))
    else {
        return Ok(completions);
    };

    let db = context.state.db();
    let Some(description) = describe_callee(db, context.file, &callee)? else {
        return Ok(completions);
    };

    for formal in description.formals {
        match completion_item_from_parameter(&formal.name, &request.callee, request.position) {
            Ok(item) => completions.push(item),
            Err(err) => log::error!("{err:?}"),
        }
    }

    // Keep signature order rather than sorting argument names alphabetically.
    set_sort_text_by_first_appearance(&mut completions);

    Ok(completions)
}

#[cfg(test)]
mod tests {
    use tower_lsp_server::ls_types::CompletionItem;

    use crate::lsp::completions::finish;
    use crate::lsp::completions::request::CallDispatch;
    use crate::lsp::completions::request::CallRequest;
    use crate::lsp::completions::request::RCompletionReplies;
    use crate::lsp::completions::tests::utils::request_at_cursor;
    use crate::lsp::completions::tests::utils::static_completions_at_cursor;
    use crate::lsp::completions::tests::utils::static_completions_with_state;
    use crate::lsp::completions::tests::utils::static_completions_with_workspace;
    use crate::lsp::completions::tests::utils::write_library_package;

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

    fn static_labels(completions: &[CompletionItem]) -> Vec<&str> {
        completions.iter().map(|item| item.label.as_str()).collect()
    }

    #[test]
    fn test_described_arguments_without_session() {
        let (completions, _) =
            static_completions_at_cursor("my_fun <- function(y, x = 1, ...) x + y\nmy_fun(@)")
                .unwrap();
        let completions = finish(completions, RCompletionReplies::default());

        assert_eq!(static_labels(&completions), vec!["y = ", "x = ", "..."]);
    }

    #[test]
    fn test_described_arguments_from_package_sources() {
        let library = tempfile::TempDir::new().unwrap();
        let mut state = write_library_package(
            library.path(),
            "pkg",
            "export(adist)\n",
            Some(&[("pkg.R", "adist <- function(x, y = NULL) NULL\n")]),
        );

        let (completions, _) = static_completions_with_state("pkg::adist(@)", &mut state);
        let completions = finish(completions, RCompletionReplies::default());

        assert_eq!(static_labels(&completions), vec!["x = ", "y = "]);
    }

    #[test]
    fn test_undescribed_callee_has_no_static_arguments() {
        // Other workspace scripts do not supply bindings without `source()`.
        let (completions, _) =
            static_completions_with_workspace("my_fun(@)", "my_fun <- function(y, x) x + y");
        let completions = finish(completions, RCompletionReplies::default());
        assert_eq!(completions, vec![]);

        // Static callee lookup does not follow aliases.
        let (completions, _) = static_completions_at_cursor(
            "my_fun <- function(y, x) x + y\nother <- my_fun\nother(@)",
        )
        .unwrap();
        let completions = finish(completions, RCompletionReplies::default());
        assert_eq!(completions, vec![]);
    }
}
