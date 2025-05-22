use std::collections::HashMap;

use tower_lsp::lsp_types::CodeAction;
use tower_lsp::lsp_types::CodeActionKind;
use tower_lsp::lsp_types::CodeActionOrCommand;
use tower_lsp::lsp_types::CodeActionResponse;
use tower_lsp::lsp_types::DocumentChanges;
use tower_lsp::lsp_types::OneOf;
use tower_lsp::lsp_types::OptionalVersionedTextDocumentIdentifier;
use tower_lsp::lsp_types::TextDocumentEdit;
use tower_lsp::lsp_types::TextEdit;
use tower_lsp::lsp_types::WorkspaceEdit;
use tree_sitter::Range;
use url::Url;

use crate::lsp::capabilities::Capabilities;
use crate::lsp::code_action::roxygen::roxygen_documentation;
use crate::lsp::documents::Document;

mod roxygen;

/// A small wrapper around [CodeActionResponse] that make a few things more ergonomic
pub(crate) struct CodeActions {
    response: CodeActionResponse,
}

pub(crate) fn code_actions(
    uri: &Url,
    document: &Document,
    range: Range,
    capabilities: &Capabilities,
) -> CodeActionResponse {
    let mut actions = CodeActions::new();

    roxygen_documentation(&mut actions, uri, document, range, capabilities);

    actions.into_response()
}

pub(crate) fn code_action(title: String, kind: CodeActionKind, edit: WorkspaceEdit) -> CodeAction {
    CodeAction {
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
    edits: Vec<TextEdit>,
    capabilities: &Capabilities,
) -> WorkspaceEdit {
    if capabilities.workspace_edit_document_changes() {
        // Prefer the versioned `DocumentChanges` feature
        let edit = TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier { uri, version },
            edits: edits.into_iter().map(|edit| OneOf::Left(edit)).collect(),
        };

        let document_changes = DocumentChanges::Edits(vec![edit]);

        WorkspaceEdit {
            changes: None,
            document_changes: Some(document_changes),
            change_annotations: None,
        }
    } else {
        // Fall back to hash map of `TextEdit`s if the client doesn't support `DocumentChanges`
        let mut changes = HashMap::new();
        changes.insert(uri, edits);

        WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        }
    }
}

impl CodeActions {
    pub(crate) fn new() -> Self {
        Self {
            response: CodeActionResponse::new(),
        }
    }

    pub(crate) fn add_action(&mut self, x: CodeAction) -> Option<()> {
        self.response.push(CodeActionOrCommand::CodeAction(x));
        Some(())
    }

    pub(crate) fn into_response(self) -> CodeActionResponse {
        self.response
    }
}
