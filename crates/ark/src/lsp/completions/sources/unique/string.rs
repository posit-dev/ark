//
// string.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use crate::lsp::completions::request::UniqueRequest;
use crate::lsp::completions::sources::unique::subset::node_find_object_for_string_subset;
use crate::lsp::completions::sources::unique::Recognized;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::node_find_string;

/// Recognizes a cursor inside a string, suppressing later sources even when
/// there are no session items.
pub(super) fn recognize_string(context: &DocumentContext) -> anyhow::Result<Option<Recognized>> {
    let node = context.node;

    // Find actual `NodeType::String` node. Needed in case we are in its children.
    let Some(node) = node_find_string(&node) else {
        return Ok(None);
    };

    // Must actually be "inside" the string, so these places don't count, even
    // though they are detected as part of the string nodes `|""|`
    if node.start_position() == context.point || node.end_position() == context.point {
        return Ok(None);
    }

    // Return empty set if we are here due to a trigger character like `$`.
    // See posit-dev/positron#1884.
    if context.trigger.is_some() {
        return Ok(Some(Recognized::Empty));
    }

    // Check if we are doing string subsetting, like `x["<tab>"]`. This is a very unique
    // case that takes priority over file path completions.
    if let Some(object) = node_find_object_for_string_subset(&node, context) {
        let object = object.node_as_str(context.contents)?.to_string();
        return Ok(Some(Recognized::Session(UniqueRequest::StringSubset {
            object,
        })));
    }

    // If no special string cases are hit, we show file path completions
    let token = node.node_as_str(context.contents)?.to_string();
    Ok(Some(Recognized::Session(UniqueRequest::FilePath { token })))
}

#[cfg(test)]
mod tests {
    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::completion_context::CompletionContext;
    use crate::lsp::completions::request::RCompletionRequest;
    use crate::lsp::completions::request::UniqueRequest;
    use crate::lsp::completions::sources::unique;
    use crate::lsp::completions::sources::unique::string::recognize_string;
    use crate::lsp::completions::sources::unique::Recognized;
    use crate::lsp::document_context::TestDocument;
    use crate::lsp::state::WorldState;
    use crate::treesitter::node_find_string;
    use crate::treesitter::NodeTypeExt;

    #[test]
    fn test_outside_quotes() {
        // Before or after the `''`, i.e. `|''` or `''|`.
        // Still considered part of the string node.
        let (text, point) = point_from_cursor("@''");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);

        assert!(node_find_string(&context.node).is_some());
        assert_eq!(recognize_string(&context).unwrap(), None);
    }

    #[test]
    fn test_not_string() {
        let (text, point) = point_from_cursor("@foo");
        let doc = TestDocument::new(&text);
        let context = doc.context(point);

        assert!(context.node.is_identifier());
        assert_eq!(recognize_string(&context).unwrap(), None);
    }

    #[test]
    fn test_trigger() {
        let (text, point) = point_from_cursor("'~/@'");
        let doc = TestDocument::new(&text);

        let context = doc.context(point);
        assert_eq!(
            recognize_string(&context).unwrap(),
            Some(Recognized::Session(UniqueRequest::FilePath {
                token: String::from("'~/'")
            }))
        );

        // `Some` trigger -> Should return empty completion set
        let context = doc.context_with_trigger(point, Some(String::from("$")));
        assert_eq!(recognize_string(&context).unwrap(), Some(Recognized::Empty));

        // Check for same result when consulting (potentially all) unique sources
        let state = WorldState::default();
        let completion_context = CompletionContext::new(&context, &state);
        let mut request = RCompletionRequest::default();
        let res = unique::get_completions(&completion_context, &mut request).unwrap();

        assert_eq!(res, Some(vec![]));
        assert_eq!(request, RCompletionRequest::default());
    }
}
