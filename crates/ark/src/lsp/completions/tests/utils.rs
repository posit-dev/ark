//
// utils.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//

use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::CompletionTextEdit;
use tree_sitter::Point;

use crate::fixtures::point_from_cursor;
use crate::lsp::completions::finish;
use crate::lsp::completions::provide::StaticCompletions;
use crate::lsp::completions::r_completions;
use crate::lsp::completions::request::RCompletionRequest;
use crate::lsp::completions::sources::utils::has_priority_prefix;
use crate::lsp::completions::static_completions;
use crate::lsp::document_context::TestDocument;
use crate::lsp::state::WorldState;

/// Runs both completion passes. Requires the R thread.
pub(crate) fn get_completions_at_cursor(cursor_text: &str) -> anyhow::Result<Vec<CompletionItem>> {
    let (completions, request) = static_completions_at_cursor(cursor_text)?;
    let replies = r_completions(request);
    Ok(finish(completions, replies))
}

/// Runs only the static pass. Composite sources require the R thread.
pub(crate) fn static_completions_at_cursor(
    cursor_text: &str,
) -> anyhow::Result<(StaticCompletions, RCompletionRequest)> {
    let (text, point) = point_from_cursor(cursor_text);
    static_completions_at(&text, point)
}

pub(crate) fn static_completions_at(
    text: &str,
    point: Point,
) -> anyhow::Result<(StaticCompletions, RCompletionRequest)> {
    let doc = TestDocument::new(text);
    let document_context = doc.context(point);
    let state = WorldState::default().snapshot();

    match static_completions(&document_context, &state) {
        Ok(completions) => Ok(completions),
        Err(err) => Err(anyhow::anyhow!("Failed to get completions: {err}")),
    }
}

/// Returns the session request built by the static pass.
pub(crate) fn request_at_cursor(cursor_text: &str) -> RCompletionRequest {
    let (_, request) = static_completions_at_cursor(cursor_text).unwrap();
    request
}

pub(crate) fn find_completion_by_label<'a>(
    completions: &'a [lsp_types::CompletionItem],
    label: &str,
) -> Option<&'a lsp_types::CompletionItem> {
    completions.iter().find(|c| c.label == label)
}

pub(crate) fn assert_text_edit(item: &lsp_types::CompletionItem, expected_text: &str) {
    assert!(item.text_edit.is_some());
    assert!(item.insert_text.is_none());

    match item.text_edit.as_ref().unwrap() {
        CompletionTextEdit::Edit(edit) => {
            assert_eq!(
                edit.new_text, expected_text,
                "Text edit should replace with '{expected_text}'"
            );
        },
        _ => panic!("Unexpected TextEdit variant"),
    }
}

pub(crate) fn assert_has_parameter_hints(item: &lsp_types::CompletionItem) {
    match &item.command {
        Some(command) => assert_eq!(command.command, "editor.action.triggerParameterHints"),
        None => panic!("CompletionItem is missing parameter hints command"),
    }
}

pub(crate) fn assert_no_command(item: &lsp_types::CompletionItem) {
    assert!(
        item.command.is_none(),
        "CompletionItem should not have an associated command"
    );
}

pub(crate) fn assert_sort_text_has_priority_prefix(item: &lsp_types::CompletionItem) {
    assert!(item.sort_text.is_some());
    let sort_text = item.sort_text.as_ref().unwrap();
    assert!(has_priority_prefix(sort_text));
}
