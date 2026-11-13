//
// handlers.rs
//
// Copyright (C) 2024-2026 Posit Software, PBC. All rights reserved.
//
//

//! Host request and event handling through [`Handlers`], with shared handlers
//! for requests that do not require an R session.

use std::fmt::Debug;

use aether_path::FilePath;
use serde_json::Value;
use stdext::result::ResultExt;
use tower_lsp_server::ls_types::CodeActionParams;
use tower_lsp_server::ls_types::CodeActionResponse;
use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::CompletionParams;
use tower_lsp_server::ls_types::CompletionResponse;
use tower_lsp_server::ls_types::DocumentOnTypeFormattingParams;
use tower_lsp_server::ls_types::DocumentSymbol;
use tower_lsp_server::ls_types::DocumentSymbolParams;
use tower_lsp_server::ls_types::DocumentSymbolResponse;
use tower_lsp_server::ls_types::FoldingRange;
use tower_lsp_server::ls_types::FoldingRangeParams;
use tower_lsp_server::ls_types::GotoDefinitionParams;
use tower_lsp_server::ls_types::GotoDefinitionResponse;
use tower_lsp_server::ls_types::Hover;
use tower_lsp_server::ls_types::HoverParams;
use tower_lsp_server::ls_types::Location;
use tower_lsp_server::ls_types::MessageType;
use tower_lsp_server::ls_types::PrepareRenameResponse;
use tower_lsp_server::ls_types::ReferenceParams;
use tower_lsp_server::ls_types::RenameParams;
use tower_lsp_server::ls_types::SelectionRange;
use tower_lsp_server::ls_types::SelectionRangeParams;
use tower_lsp_server::ls_types::ServerCapabilities;
use tower_lsp_server::ls_types::ServerInfo;
use tower_lsp_server::ls_types::SignatureHelp;
use tower_lsp_server::ls_types::SignatureHelpParams;
use tower_lsp_server::ls_types::SymbolInformation;
use tower_lsp_server::ls_types::TextDocumentPositionParams;
use tower_lsp_server::ls_types::TextEdit;
use tower_lsp_server::ls_types::WorkspaceEdit;
use tower_lsp_server::ls_types::WorkspaceSymbolParams;
use tower_lsp_server::Client;

use crate::lsp;
use crate::lsp::backend::LspResult;
use crate::lsp::capabilities::Capabilities;
use crate::lsp::code_action::code_actions;
use crate::lsp::coordinates::lsp_range_from_tree_sitter_range;
use crate::lsp::coordinates::tree_sitter_point_from_lsp_position;
use crate::lsp::coordinates::tree_sitter_range_from_lsp_range;
use crate::lsp::db::FileLegacyExt;
use crate::lsp::find_references::find_references;
use crate::lsp::folding_range::folding_range;
use crate::lsp::goto_definition::goto_definition;
use crate::lsp::indent::indent_edit;
use crate::lsp::main_loop::Readiness;
use crate::lsp::rename;
use crate::lsp::selection_range::convert_selection_range_from_tree_sitter_to_lsp;
use crate::lsp::selection_range::selection_range;
use crate::lsp::state::WorldState;
use crate::lsp::symbols;
use crate::lsp::traits::url::UriExt;

/// Host hooks for requests, events, initialization and diagnostics.
/// Request handlers run inside `respond()`'s panic and cancellation recovery.
/// A host can override individual handlers while keeping the shared defaults.
///
/// Completion, hover and signature help return no result by default.
/// Completion resolve returns the item unchanged. Ark overrides these methods
/// to consult its R session for documentation and session names. The other
/// request methods with defaults call shared handlers.
pub trait Handlers: Debug + Send + 'static {
    /// Requests only this host serves, sent by the custom methods it registers
    /// on the service builder.
    type Request: Debug + Send;
    type Response: Debug + Send;

    /// Events the host posts into the main loop, handled by [`Self::event()`].
    type Event: Debug + Send;

    fn server_info(&self) -> ServerInfo;

    /// Adjust the capabilities advertised in the `initialize` response.
    fn capabilities(&self, base: ServerCapabilities) -> ServerCapabilities {
        base
    }

    fn workspace_symbol(
        &self,
        params: WorkspaceSymbolParams,
        cx: &RequestContext<'_>,
    ) -> LspResult<Option<Vec<SymbolInformation>>> {
        handle_symbol(params, cx.world)
    }

    fn document_symbol(
        &self,
        params: DocumentSymbolParams,
        cx: &RequestContext<'_>,
    ) -> LspResult<Option<DocumentSymbolResponse>> {
        handle_document_symbol(params, cx.world)
    }

    fn folding_range(
        &self,
        params: FoldingRangeParams,
        cx: &RequestContext<'_>,
    ) -> LspResult<Option<Vec<FoldingRange>>> {
        handle_folding_range(params, cx.world)
    }

    fn completion(
        &self,
        _params: CompletionParams,
        _cx: &RequestContext<'_>,
    ) -> LspResult<Option<CompletionResponse>> {
        Ok(None)
    }

    fn completion_resolve(
        &self,
        item: CompletionItem,
        _cx: &RequestContext<'_>,
    ) -> LspResult<CompletionItem> {
        Ok(item)
    }

    fn hover(&self, _params: HoverParams, _cx: &RequestContext<'_>) -> LspResult<Option<Hover>> {
        Ok(None)
    }

    fn signature_help(
        &self,
        _params: SignatureHelpParams,
        _cx: &RequestContext<'_>,
    ) -> LspResult<Option<SignatureHelp>> {
        Ok(None)
    }

    fn goto_definition(
        &self,
        params: GotoDefinitionParams,
        cx: &RequestContext<'_>,
    ) -> LspResult<Option<GotoDefinitionResponse>> {
        handle_goto_definition(params, cx.world)
    }

    fn selection_range(
        &self,
        params: SelectionRangeParams,
        cx: &RequestContext<'_>,
    ) -> LspResult<Option<Vec<SelectionRange>>> {
        handle_selection_range(params, cx.world)
    }

    fn references(
        &self,
        params: ReferenceParams,
        cx: &RequestContext<'_>,
    ) -> LspResult<Option<Vec<Location>>> {
        handle_references(params, cx.world)
    }

    fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
        cx: &RequestContext<'_>,
    ) -> LspResult<Option<PrepareRenameResponse>> {
        handle_prepare_rename(params, cx.world)
    }

    fn rename(
        &self,
        params: RenameParams,
        cx: &RequestContext<'_>,
    ) -> LspResult<Option<WorkspaceEdit>> {
        handle_rename(params, cx.world)
    }

    /// The main loop applies the request's indentation options before calling
    /// this method, unless the document's indent size and tab width differ.
    /// `cx` exposes the resulting document configuration.
    fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
        cx: &RequestContext<'_>,
    ) -> LspResult<Option<Vec<TextEdit>>> {
        handle_indent(params, cx.world)
    }

    fn code_action(
        &self,
        params: CodeActionParams,
        cx: &RequestContext<'_>,
    ) -> LspResult<Option<CodeActionResponse>> {
        handle_code_action(params, cx.capabilities, cx.world)
    }

    fn request(&self, request: Self::Request, cx: &RequestContext<'_>)
        -> LspResult<Self::Response>;

    /// Handle a host event inside the main loop's event boundary. This is the
    /// only host method with write access to the world state.
    fn event(&mut self, event: Self::Event, cx: &mut EventContext<'_>) -> anyhow::Result<()>;

    /// Called after the main loop applies an editor change to the document at
    /// `path`.
    fn did_change_document(&mut self, _path: &FilePath) {}

    /// Whether the open document at `path` gets diagnostics.
    fn should_diagnose(&self, _path: &FilePath) -> bool {
        true
    }
}

/// Read-only state for answering a request.
pub struct RequestContext<'a> {
    pub(crate) world: &'a WorldState,
    pub(crate) capabilities: &'a Capabilities,
    pub(crate) readiness: Readiness<'a>,
}

/// State that a host event may write.
pub struct EventContext<'a> {
    pub(crate) world: &'a mut WorldState,
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_symbol(
    params: WorkspaceSymbolParams,
    state: &WorldState,
) -> LspResult<Option<Vec<SymbolInformation>>> {
    symbols::symbols(
        state.db(),
        &params,
        state.config.position_encoding,
        state.config.workspace_symbols.include_comment_sections,
        |file| state.wire_uri(file),
    )
    .map(Some)
    .or_else(|err| {
        // Missing doc: Why are we not propagating errors to the frontend?
        lsp::log_error!("{err:?}");
        Ok(None)
    })
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_document_symbol(
    params: DocumentSymbolParams,
    state: &WorldState,
) -> LspResult<Option<DocumentSymbolResponse>> {
    document_symbols(&params, state)
        .map(|res| Some(DocumentSymbolResponse::Nested(res)))
        .or_else(|err| {
            // Missing doc: Why are we not propagating errors to the frontend?
            lsp::log_error!("{err:?}");
            Ok(None)
        })
}

/// Document lookup errors reach `handle_document_symbol()`, which logs them
/// and returns `None`. `symbols::document_symbols()` logs collection errors
/// itself and returns an empty symbol list.
fn document_symbols(
    params: &DocumentSymbolParams,
    state: &WorldState,
) -> anyhow::Result<Vec<DocumentSymbol>> {
    let path = params.text_document.uri.to_document_path()?;
    let file = state.open_file(&path)?.file();
    symbols::document_symbols(
        state.db(),
        file,
        state.config.position_encoding,
        state.config.symbols.include_assignments_in_blocks,
    )
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_folding_range(
    params: FoldingRangeParams,
    state: &WorldState,
) -> LspResult<Option<Vec<FoldingRange>>> {
    let path = params.text_document.uri.to_document_path()?;
    let file = state.open_file(&path)?.file();
    let db = state.db();
    match folding_range(db, file) {
        Ok(foldings) => Ok(Some(foldings)),
        Err(err) => {
            lsp::log_error!("{err:?}");
            Ok(None)
        },
    }
}

pub(crate) async fn handle_execute_command(client: &Client) -> LspResult<Option<Value>> {
    match client.apply_edit(WorkspaceEdit::default()).await {
        Ok(res) if res.applied => client.log_message(MessageType::INFO, "applied").await,
        Ok(_) => client.log_message(MessageType::INFO, "rejected").await,
        Err(err) => client.log_message(MessageType::ERROR, err).await,
    }
    Ok(None)
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_goto_definition(
    params: GotoDefinitionParams,
    state: &WorldState,
) -> LspResult<Option<GotoDefinitionResponse>> {
    Ok(goto_definition(params, state).log_err().flatten())
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_selection_range(
    params: SelectionRangeParams,
    state: &WorldState,
) -> LspResult<Option<Vec<SelectionRange>>> {
    let path = params.text_document.uri.to_document_path()?;
    let file = state.open_file(&path)?.file();
    let db = state.db();
    let encoding = state.config.position_encoding;

    // Get tree-sitter points to return selection ranges for
    let points = params
        .positions
        .into_iter()
        .map(|position| {
            tree_sitter_point_from_lsp_position(position, file.line_index(db), encoding)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let Some(selections) = selection_range(file.tree_sitter(db), points) else {
        return Ok(None);
    };

    // Convert tree-sitter points to LSP positions everywhere
    let selections = selections
        .into_iter()
        .map(|selection| {
            convert_selection_range_from_tree_sitter_to_lsp(db, file, encoding, selection)
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(Some(selections))
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_references(
    params: ReferenceParams,
    state: &WorldState,
) -> LspResult<Option<Vec<Location>>> {
    let locations = match find_references(params, state) {
        Ok(locations) => locations,
        Err(_error) => {
            return Ok(None);
        },
    };

    if locations.is_empty() {
        Ok(None)
    } else {
        Ok(Some(locations))
    }
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_prepare_rename(
    params: TextDocumentPositionParams,
    state: &WorldState,
) -> LspResult<Option<PrepareRenameResponse>> {
    Ok(rename::prepare_rename(params, state)?)
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_rename(
    params: RenameParams,
    state: &WorldState,
) -> LspResult<Option<WorkspaceEdit>> {
    // Propagate error to the frontend to give actionable feedback to the user.
    // All errors thrown by `rename()` must be informative for users.
    Ok(rename::rename(params, state)?)
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_indent(
    params: DocumentOnTypeFormattingParams,
    state: &WorldState,
) -> LspResult<Option<Vec<TextEdit>>> {
    let ctxt = params.text_document_position;
    let path = ctxt.text_document.uri.to_document_path()?;
    let open_file = state.open_file(&path)?;
    let encoding = state.config.position_encoding;

    let db = state.db();
    let line_index = open_file.line_index(db);
    let point = tree_sitter_point_from_lsp_position(ctxt.position, line_index, encoding)?;

    let Some(edits) = indent_edit(db, open_file.file(), &open_file.config().indent, point.row)?
    else {
        return Ok(None);
    };

    let edits = edits
        .into_iter()
        .map(|edit| {
            Ok(TextEdit {
                range: lsp_range_from_tree_sitter_range(edit.range, line_index, encoding)?,
                new_text: edit.new_text,
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(Some(edits))
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_code_action(
    params: CodeActionParams,
    capabilities: &Capabilities,
    state: &WorldState,
) -> LspResult<Option<CodeActionResponse>> {
    let path = params.text_document.uri.to_document_path()?;
    let file = state.open_file(&path)?;
    let db = state.db();
    let encoding = state.config.position_encoding;
    let range = tree_sitter_range_from_lsp_range(params.range, file.line_index(db), encoding)?;

    let actions = code_actions(
        db,
        file.file(),
        range,
        capabilities.code_action_literal_support(),
    );
    let response = actions.into_response(
        db,
        file.file(),
        file.wire_uri(),
        file.version(),
        encoding,
        capabilities.workspace_edit_document_changes(),
    );

    if response.is_empty() {
        Ok(None)
    } else {
        Ok(Some(response))
    }
}
