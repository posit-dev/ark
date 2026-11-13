//
// custom.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use legacy_ide::treesitter::node_in_string;

use crate::lsp::call_arguments::call_arguments_at;
use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::request::ArgumentPosition;
use crate::lsp::completions::request::CustomRequest;
use crate::lsp::completions::sources::utils::call_node_position_type;
use crate::lsp::completions::sources::utils::CallNodePositionType;

/// Collects what the session needs to look up custom completion callbacks for
/// the argument under the cursor. Whether a callback applies is only known in
/// the session.
pub(super) fn custom_request(context: &CompletionContext) -> anyhow::Result<Option<CustomRequest>> {
    if context.containing_call_node().is_none() {
        // Didn't detect anything worth completing in this context,
        // let other sources add their own candidates instead
        return Ok(None);
    }

    let document_context = context.document_context;
    let point = document_context.point;
    let node = document_context.node;

    let Some(arguments) = call_arguments_at(document_context)? else {
        return Ok(None);
    };

    // Check and see if we're in the 'name' position,
    // versus the 'value' position, for a function invocation.
    //
    // For example:
    //
    //    Sys.setenv(EDITOR = "vim")
    //    ^^^^^^^^^^ ^^^^^^   ^^^^^ ^
    //    other      name     value other
    //
    // This is mainly relevant because we might only want to
    // provide certain completions in the 'name' position.
    let position = match call_node_position_type(&node, point) {
        CallNodePositionType::Name => ArgumentPosition::Name,
        // Currently mapping ambiguous `fn(arg<tab>)` to `"name"`, but we could
        // return `"ambiguous"` and allow our handlers to handle this individually
        CallNodePositionType::Ambiguous => ArgumentPosition::Name,
        CallNodePositionType::Value => ArgumentPosition::Value,
        CallNodePositionType::Outside => {
            // Call detected, but on the RHS of a `)` node or the LHS
            // of a `(` node, i.e. outside the parenthesis.
            return Ok(None);
        },
        CallNodePositionType::Unknown => {
            // Call detected, but inside some very odd edge case
            return Ok(None);
        },
    };

    Ok(Some(CustomRequest {
        arguments,
        position,
        in_string: node_in_string(&node),
    }))
}

#[cfg(test)]
mod tests {
    use legacy_ide::fixtures::point_from_cursor;

    use crate::lsp::call_arguments::CallArguments;
    use crate::lsp::completions::completion_context::CompletionContext;
    use crate::lsp::completions::request::ArgumentPosition;
    use crate::lsp::completions::request::CustomRequest;
    use crate::lsp::completions::sources::unique::custom::custom_request;
    use crate::lsp::document_context::TestDocument;

    fn custom_request_at_cursor(code: &str) -> Option<CustomRequest> {
        let (text, point) = point_from_cursor(code);
        let doc = TestDocument::new(&text);
        let document_context = doc.context(point);
        let context = CompletionContext::new(&document_context, doc.file(), doc.state());

        custom_request(&context).unwrap()
    }

    #[test]
    fn test_custom_request() {
        assert_eq!(
            custom_request_at_cursor("Sys.setenv(AAA = @)"),
            Some(CustomRequest {
                arguments: CallArguments {
                    callee: String::from("Sys.setenv"),
                    explicit_names: vec![String::from("AAA")],
                    unnamed_count: -1,
                    active_name: Some(String::from("AAA")),
                },
                position: ArgumentPosition::Value,
                in_string: false,
            })
        );
    }

    #[test]
    fn test_no_custom_request_outside_call() {
        assert_eq!(custom_request_at_cursor("foo@"), None);
        assert_eq!(custom_request_at_cursor("Sys.getenv()@"), None);
    }
}
