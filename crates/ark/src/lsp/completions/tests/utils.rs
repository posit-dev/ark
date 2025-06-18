//
// completions/tests/utils.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//

use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionTextEdit;

use crate::fixtures::utils::point_from_cursor;
use crate::lsp::completions::provide_completions;
use crate::lsp::completions::sources::utils::has_priority_prefix;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::documents::Document;
use crate::lsp::state::WorldState;

pub(crate) fn get_completions_at_cursor(cursor_text: &str) -> anyhow::Result<Vec<CompletionItem>> {
    let (text, point) = point_from_cursor(cursor_text);
    let document = Document::new(&text, None);
    let document_context = DocumentContext::new(&document, point, None);
    let state = WorldState::default();

    match provide_completions(&document_context, &state) {
        Ok(completions) => Ok(completions),
        Err(err) => Err(anyhow::anyhow!("Failed to get completions: {err}")),
    }
}

pub(crate) fn find_completion_by_label<'a>(
    completions: &'a [tower_lsp::lsp_types::CompletionItem],
    label: &str,
) -> &'a tower_lsp::lsp_types::CompletionItem {
    completions
        .iter()
        .find(|c| c.label == label)
        .unwrap_or_else(|| panic!("Completion item with label '{label}' not found"))
}

pub(crate) fn assert_text_edit(item: &tower_lsp::lsp_types::CompletionItem, expected_text: &str) {
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

pub(crate) fn assert_has_parameter_hints(item: &tower_lsp::lsp_types::CompletionItem) {
    match &item.command {
        Some(command) => assert_eq!(command.command, "editor.action.triggerParameterHints"),
        None => panic!("CompletionItem is missing parameter hints command"),
    }
}

pub(crate) fn assert_no_command(item: &tower_lsp::lsp_types::CompletionItem) {
    assert!(
        item.command.is_none(),
        "CompletionItem should not have an associated command"
    );
}

pub(crate) fn assert_sort_text_has_priority_prefix(item: &tower_lsp::lsp_types::CompletionItem) {
    assert!(item.sort_text.is_some());
    let sort_text = item.sort_text.as_ref().unwrap();
    assert!(has_priority_prefix(sort_text));
}
