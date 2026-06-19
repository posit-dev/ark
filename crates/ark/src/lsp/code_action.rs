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
use oak_db::Db;
use oak_db::File;
use tower_lsp::lsp_types;
use tree_sitter::Point;
use tree_sitter::Range;
use url::Url;

use crate::lsp::capabilities::Capabilities;
use crate::lsp::db::ArkDb;
use crate::lsp::open_file::lsp_position_from_tree_sitter_point;
use crate::lsp::open_file::OpenFile;

mod roxygen;

/// A code action computed from analysis, in tree-sitter coordinates and without
/// the editor identity. [`into_response`] converts the coordinates with the
/// position encoding and injects the document URL and version at the LSP boundary.
pub(crate) struct CodeActionEdit {
    title: String,
    kind: lsp_types::CodeActionKind,
    edits: Vec<CodeActionTextEdit>,
}

/// One text edit of a [`CodeActionEdit`], in tree-sitter coordinates.
pub(crate) struct CodeActionTextEdit {
    start: Point,
    end: Point,
    new_text: String,
}

impl CodeActionEdit {
    pub(crate) fn new(
        title: String,
        kind: lsp_types::CodeActionKind,
        edits: Vec<CodeActionTextEdit>,
    ) -> Self {
        Self { title, kind, edits }
    }
}

impl CodeActionTextEdit {
    /// A zero-width insertion of `new_text` at `point`.
    pub(crate) fn insertion(point: Point, new_text: String) -> Self {
        Self {
            start: point,
            end: point,
            new_text,
        }
    }
}

/// Accumulates the code actions for a request. Holds analysis-layer edits in
/// tree-sitter coordinates; [`CodeActions::into_response`] applies the editor
/// identity and position encoding to assemble the LSP response.
pub(crate) struct CodeActions {
    actions: Vec<CodeActionEdit>,
}

/// Compute the code actions for `range`.
pub(crate) fn code_actions(
    db: &dyn ArkDb,
    file: File,
    range: Range,
    capabilities: &Capabilities,
) -> CodeActions {
    let mut actions = CodeActions::new();

    if let Some(action) = roxygen::to_code_action(db, file, range, capabilities) {
        actions.add_action(action);
    }

    actions
}

impl CodeActions {
    pub(crate) fn new() -> Self {
        Self {
            actions: Vec::new(),
        }
    }

    pub(crate) fn add_action(&mut self, action: CodeActionEdit) {
        self.actions.push(action);
    }

    /// Assemble into the LSP response. This is the boundary where the `OpenFile`
    /// and the position encoding enter. It converts the tree-sitter coordinates
    /// into LSP `TextEdit`s and stamps each edit with the document's wire URL
    /// and version.
    pub(crate) fn into_response(
        self,
        db: &dyn Db,
        file: &OpenFile,
        encoding: PositionEncoding,
        capabilities: &Capabilities,
    ) -> lsp_types::CodeActionResponse {
        let line_index = file.line_index(db);

        self.actions
            .into_iter()
            .filter_map(|action| {
                let edits = action
                    .edits
                    .into_iter()
                    .map(|edit| {
                        let start =
                            lsp_position_from_tree_sitter_point(edit.start, line_index, encoding)?;
                        let end =
                            lsp_position_from_tree_sitter_point(edit.end, line_index, encoding)?;
                        Ok(lsp_types::TextEdit::new(
                            lsp_types::Range::new(start, end),
                            edit.new_text,
                        ))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()
                    .ok()?;

                let workspace_edit = code_action_workspace_text_edit(
                    file.wire_url().clone(),
                    file.version(),
                    edits,
                    capabilities,
                );

                Some(lsp_types::CodeActionOrCommand::CodeAction(code_action(
                    action.title,
                    action.kind,
                    workspace_edit,
                )))
            })
            .collect()
    }
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
