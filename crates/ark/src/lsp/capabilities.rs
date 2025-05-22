//
// capabilities.rs
//
// Copyright (C) 2025 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp::lsp_types;
use tower_lsp::lsp_types::CodeActionKind;
use tower_lsp::lsp_types::CodeActionOptions;
use tower_lsp::lsp_types::CodeActionProviderCapability;
use tower_lsp::lsp_types::WorkDoneProgressOptions;

/// Capabilities negotiated with [lsp_types::ClientCapabilities]
#[derive(Debug)]
pub(crate) struct Capabilities {
    dynamic_registration_for_did_change_configuration: bool,
    code_action_literal_support: bool,
    workspace_edit_document_changes: bool,
}

impl Capabilities {
    pub(crate) fn new(client_capabilities: lsp_types::ClientCapabilities) -> Self {
        let dynamic_registration_for_did_change_configuration = client_capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.did_change_configuration)
            .and_then(|did_change_configuration| did_change_configuration.dynamic_registration)
            .unwrap_or(false);

        // In theory the client also tells us which code action kinds it supports inside
        // `code_action_literal_support`, but clients are guaranteed to ignore any they
        // don't support, so we just return `true` if the field exists (same as
        // rust-analyzer).
        let code_action_literal_support = client_capabilities
            .text_document
            .as_ref()
            .and_then(|text_document| text_document.code_action.as_ref())
            .and_then(|code_action| code_action.code_action_literal_support.as_ref())
            .map_or(false, |_| true);

        let workspace_edit_document_changes = client_capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.workspace_edit.as_ref())
            .and_then(|workspace_edit| workspace_edit.document_changes)
            .map_or(false, |document_changes| document_changes);

        Self {
            dynamic_registration_for_did_change_configuration,
            code_action_literal_support,
            workspace_edit_document_changes,
        }
    }

    pub(crate) fn dynamic_registration_for_did_change_configuration(&self) -> bool {
        self.dynamic_registration_for_did_change_configuration
    }

    pub(crate) fn code_action_literal_support(&self) -> bool {
        self.code_action_literal_support
    }

    // Currently only used for testing
    #[cfg(test)]
    pub(crate) fn with_code_action_literal_support(
        mut self,
        code_action_literal_support: bool,
    ) -> Self {
        self.code_action_literal_support = code_action_literal_support;
        return self;
    }

    pub(crate) fn workspace_edit_document_changes(&self) -> bool {
        self.workspace_edit_document_changes
    }

    // Currently only used for testing
    #[cfg(test)]
    pub(crate) fn with_workspace_edit_document_changes(
        mut self,
        workspace_edit_document_changes: bool,
    ) -> Self {
        self.workspace_edit_document_changes = workspace_edit_document_changes;
        return self;
    }

    pub(crate) fn code_action_provider_capability(&self) -> Option<CodeActionProviderCapability> {
        if !self.code_action_literal_support() {
            return None;
        }

        // Currently we only support documentation generating code actions, which don't
        // map to an existing kind. rust-analyzer maps them to `EMPTY`, so we follow suit.
        // Currently no code actions require delayed resolution.
        Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![CodeActionKind::EMPTY]),
            work_done_progress_options: WorkDoneProgressOptions::default(),
            resolve_provider: Some(false),
        }))
    }
}

// This is unfortunately required right now, because `LspState` is initialized before we
// get the `Initialize` LSP request. We immediately overwrite the `LspState`
// `capabilities` field after receiving the `Initialize` request.
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            dynamic_registration_for_did_change_configuration: false,
            code_action_literal_support: false,
            workspace_edit_document_changes: false,
        }
    }
}
