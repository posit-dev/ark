//! Code actions
//!
//! These are contextual light bulbs that appear when the user's cursor is at a particular
//! position. They allow for small context specific quick fixes, refactors, documentation
//! generation, and other small code adjustments.
//!
//! Modeled after rust-analyzer's blog post:
//! https://rust-analyzer.github.io/blog/2020/09/28/how-to-make-a-light-bulb.html

use std::collections::HashMap;

use aether_lsp_utils::proto::PositionEncoding;
use tower_lsp::lsp_types;
use tree_sitter::Range;
use url::Url;

use crate::lsp::capabilities::Capabilities;
use crate::lsp::code_action::roxygen::roxygen_documentation;
use crate::lsp::db::ArkDb;
use crate::lsp::open_file::lsp_position_from_tree_sitter_point;
use crate::lsp::open_file::OpenFile;

mod roxygen;

/// A small wrapper around [CodeActionResponse] that make a few things more ergonomic
pub(crate) struct CodeActions {
    response: lsp_types::CodeActionResponse,
}

/// Collect the code actions for `range` and assemble them into the LSP response.
pub(crate) fn code_actions(
    db: &dyn ArkDb,
    file: &OpenFile,
    range: Range,
    encoding: PositionEncoding,
    capabilities: &Capabilities,
) -> lsp_types::CodeActionResponse {
    let mut actions = CodeActions::new();

    // Our code actions return literal `CodeAction`s, so bail if the client
    // can't accept them.
    if !capabilities.code_action_literal_support() {
        return actions.into_response();
    }

    if let Some(edit) = roxygen_documentation(db, file.inner, range) {
        if let Ok(position) =
            lsp_position_from_tree_sitter_point(edit.position, file.inner.line_index(db), encoding)
        {
            let range = lsp_types::Range::new(position, position);
            let text_edit = lsp_types::TextEdit::new(range, edit.documentation);
            let workspace_edit = code_action_workspace_text_edit(
                file.wire_url.clone(),
                file.version,
                vec![text_edit],
                capabilities,
            );
            actions.add_action(code_action(
                "Generate a roxygen template".to_string(),
                lsp_types::CodeActionKind::EMPTY,
                workspace_edit,
            ));
        }
    }

    actions.into_response()
}

pub(crate) fn code_action(
    title: String,
    kind: lsp_types::CodeActionKind,
    edit: lsp_types::WorkspaceEdit,
) -> lsp_types::CodeAction {
    lsp_types::CodeAction {
        title,
        kind: Some(kind),
        edit: Some(edit),
        diagnostics: None,
        command: None,
        is_preferred: None,
        disabled: None,
        data: None,
    }
}

/// Creates a common kind of `WorkspaceEdit` composed of one or more `TextEdit`s to
/// apply to a single document
pub(crate) fn code_action_workspace_text_edit(
    uri: Url,
    version: Option<i32>,
    edits: Vec<lsp_types::TextEdit>,
    capabilities: &Capabilities,
) -> lsp_types::WorkspaceEdit {
    if capabilities.workspace_edit_document_changes() {
        // Prefer the versioned `DocumentChanges` feature
        let edit = lsp_types::TextDocumentEdit {
            text_document: lsp_types::OptionalVersionedTextDocumentIdentifier { uri, version },
            edits: edits.into_iter().map(lsp_types::OneOf::Left).collect(),
        };

        let document_changes = lsp_types::DocumentChanges::Edits(vec![edit]);

        lsp_types::WorkspaceEdit {
            changes: None,
            document_changes: Some(document_changes),
            change_annotations: None,
        }
    } else {
        // Fall back to hash map of `TextEdit`s if the client doesn't support `DocumentChanges`
        let mut changes = HashMap::new();
        changes.insert(uri, edits);

        lsp_types::WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }
    }
}

impl CodeActions {
    pub(crate) fn new() -> Self {
        Self {
            response: lsp_types::CodeActionResponse::new(),
        }
    }

    pub(crate) fn add_action(&mut self, x: lsp_types::CodeAction) -> Option<()> {
        self.response
            .push(lsp_types::CodeActionOrCommand::CodeAction(x));
        Some(())
    }

    pub(crate) fn into_response(self) -> lsp_types::CodeActionResponse {
        self.response
    }
}
