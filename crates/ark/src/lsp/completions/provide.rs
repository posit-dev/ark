//
// provide.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp_server::ls_types::CompletionItem;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::request::RCompletionReplies;
use crate::lsp::completions::request::RCompletionRequest;
use crate::lsp::completions::request::SlotReply;
use crate::lsp::completions::sources::composite;
use crate::lsp::completions::sources::composite::CompositeCompletions;
use crate::lsp::completions::sources::unique;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::state::WorldState;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::NodeTypeExt;

/// Completion items and source selection from [static_completions()], to be
/// merged with the session's replies by [finish()].
#[derive(Debug)]
pub(crate) struct StaticCompletions {
    answer: StaticAnswer,
}

#[derive(Debug)]
enum StaticAnswer {
    /// A unique source ended the search. The request's unique slot, if any,
    /// belongs to this source.
    Unique(Vec<CompletionItem>),
    /// No unique source applies. The request's composite slots are merged
    /// with these items.
    Composite(CompositeCompletions),
}

/// Recognizes the completion context, collects items, and builds an owned
/// [RCompletionRequest] for session-dependent sources.
///
/// Doesn't call R, so it can run while R is busy.
pub(crate) fn static_completions(
    document_context: &DocumentContext,
    state: &WorldState,
) -> anyhow::Result<(StaticCompletions, RCompletionRequest)> {
    log::info!(
        "static_completions() - Completion node text: '{node_text}', Node type: '{node_type:?}'",
        node_text = document_context
            .node
            .node_as_str(document_context.contents)
            .unwrap_or_default(),
        node_type = document_context.node.node_type()
    );

    let completion_context = CompletionContext::new(document_context, state);
    let mut request = RCompletionRequest::default();

    // Try unique sources first
    let answer = match unique::get_completions(&completion_context, &mut request)? {
        Some(items) => StaticAnswer::Unique(items),
        // At this point we aren't in a "unique" completion case, so just return a
        // set of reasonable completions from composite sources
        None => {
            let (completions, composite) = composite::get_completions(&completion_context)?;
            request.composite = composite;
            StaticAnswer::Composite(completions)
        },
    };

    Ok((StaticCompletions { answer }, request))
}

/// Merges the session's replies into the static completions.
pub(crate) fn finish(
    completions: StaticCompletions,
    replies: RCompletionReplies,
) -> Vec<CompletionItem> {
    let RCompletionReplies {
        custom,
        unique,
        composite,
    } = replies;

    // Custom callbacks take priority over `$`, `@`, and composite sources,
    // including when the callback returns no items.
    if let SlotReply::Fill(items) = custom {
        return items;
    }

    match completions.answer {
        StaticAnswer::Unique(items) => unique.items_or(items),
        StaticAnswer::Composite(completions) => completions.merge(composite),
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp_server::ls_types::CompletionItemKind;

    use crate::fixtures::point_and_offset_from_cursor;
    use crate::lsp::completions::finish;
    use crate::lsp::completions::request::ExtractOperator;
    use crate::lsp::completions::request::RCompletionReplies;
    use crate::lsp::completions::request::UniqueRequest;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::static_completions_at;
    use crate::lsp::completions::tests::utils::static_completions_at_cursor;

    #[test]
    fn test_dollar_is_empty_without_session() {
        let (completions, request) = static_completions_at_cursor("foo$@").unwrap();

        assert_eq!(
            request.unique,
            Some(UniqueRequest::Extractor {
                object: String::from("foo"),
                operator: ExtractOperator::Dollar,
            })
        );
        assert_eq!(finish(completions, RCompletionReplies::default()), vec![]);
    }

    #[test]
    fn test_at_is_empty_without_session() {
        // `@` is the default cursor marker.
        let (text, point, _) = point_and_offset_from_cursor("foo@|", b'|');
        let (completions, request) = static_completions_at(&text, point).unwrap();

        assert_eq!(
            request.unique,
            Some(UniqueRequest::Extractor {
                object: String::from("foo"),
                operator: ExtractOperator::At,
            })
        );
        assert_eq!(finish(completions, RCompletionReplies::default()), vec![]);
    }

    #[test]
    fn test_string_subset_lists_no_files_without_session() {
        let (completions, request) = static_completions_at_cursor(r#"x["@"]"#).unwrap();

        // String subsetting suppresses file paths even without session replies.
        assert_eq!(
            request.unique,
            Some(UniqueRequest::StringSubset {
                object: String::from("x"),
            })
        );
        assert_eq!(finish(completions, RCompletionReplies::default()), vec![]);
    }

    #[test]
    fn test_call_argument_reaches_composite_sources_without_session() {
        let (completions, request) =
            static_completions_at_cursor("arg_value <- 1\nf(arg@)").unwrap();

        // Only the session can determine whether a custom callback applies.
        assert!(request.custom.is_some());
        assert_eq!(request.unique, None);
        assert!(request.composite.call.is_some());
        assert!(request.composite.search_path.is_some());

        let completions = finish(completions, RCompletionReplies::default());
        let item = find_completion_by_label(&completions, "arg_value").unwrap();
        assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));
    }

    #[test]
    fn test_composite_without_session() {
        let (completions, request) = static_completions_at_cursor("@").unwrap();

        assert_eq!(request.composite.call, None);
        assert_eq!(request.composite.pipe_root, None);
        assert_eq!(request.composite.subset, None);
        assert!(request.composite.search_path.is_some());

        // Keywords are static
        let completions = finish(completions, RCompletionReplies::default());
        let item = find_completion_by_label(&completions, "NULL").unwrap();
        assert_eq!(item.kind, Some(CompletionItemKind::KEYWORD));
    }

    #[test]
    fn test_static_answers_have_no_session_slots() {
        let (text, point, _) = point_and_offset_from_cursor("#' @|", b'|');
        let (_, request) = static_completions_at(&text, point).unwrap();
        assert!(!request.has_session_slots());

        let (_, request) = static_completions_at_cursor("foo$@").unwrap();
        assert!(request.has_session_slots());
    }

    /// See `test_custom_overrides_dollar()` for the session's answer.
    #[test]
    fn test_dollar_answers_when_custom_declines() {
        let (completions, request) = static_completions_at_cursor("Sys.getenv(foo$@)").unwrap();

        assert!(request.custom.is_some());
        assert_eq!(
            request.unique,
            Some(UniqueRequest::Extractor {
                object: String::from("foo"),
                operator: ExtractOperator::Dollar,
            })
        );
        assert_eq!(finish(completions, RCompletionReplies::default()), vec![]);
    }
}
