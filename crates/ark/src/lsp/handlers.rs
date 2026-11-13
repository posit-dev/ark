//
// handlers.rs
//
// Copyright (C) 2024-2026 Posit Software, PBC. All rights reserved.
//
//

use anyhow::anyhow;
use serde_json::Value;
use stdext::result::ResultExt;
use stdext::unwrap;
use tower_lsp_server::ls_types::CodeActionParams;
use tower_lsp_server::ls_types::CodeActionResponse;
use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::CompletionList;
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
use tower_lsp_server::ls_types::HoverContents;
use tower_lsp_server::ls_types::HoverParams;
use tower_lsp_server::ls_types::Location;
use tower_lsp_server::ls_types::MessageType;
use tower_lsp_server::ls_types::PrepareRenameResponse;
use tower_lsp_server::ls_types::ReferenceParams;
use tower_lsp_server::ls_types::RenameParams;
use tower_lsp_server::ls_types::SelectionRange;
use tower_lsp_server::ls_types::SelectionRangeParams;
use tower_lsp_server::ls_types::SignatureHelp;
use tower_lsp_server::ls_types::SignatureHelpParams;
use tower_lsp_server::ls_types::SymbolInformation;
use tower_lsp_server::ls_types::TextDocumentPositionParams;
use tower_lsp_server::ls_types::TextEdit;
use tower_lsp_server::ls_types::WorkspaceEdit;
use tower_lsp_server::ls_types::WorkspaceSymbolParams;
use tower_lsp_server::Client;

use crate::analysis::input_boundaries::input_boundaries;
use crate::lsp;
use crate::lsp::backend::LspError;
use crate::lsp::backend::LspResult;
use crate::lsp::code_action::code_actions;
use crate::lsp::completions::finish;
use crate::lsp::completions::r_completions;
use crate::lsp::completions::resolve_completion;
use crate::lsp::completions::resolve_target;
use crate::lsp::completions::static_completions;
use crate::lsp::completions::RCompletionReplies;
use crate::lsp::coordinates::lsp_range_from_tree_sitter_range;
use crate::lsp::coordinates::tree_sitter_point_from_lsp_position;
use crate::lsp::coordinates::tree_sitter_range_from_lsp_range;
use crate::lsp::db::FileArkExt;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::find_references::find_references;
use crate::lsp::folding_range::folding_range;
use crate::lsp::goto_definition::goto_definition;
use crate::lsp::help_topic::help_topic;
use crate::lsp::help_topic::HelpTopicParams;
use crate::lsp::help_topic::HelpTopicResponse;
use crate::lsp::hover::r_hover;
use crate::lsp::indent::indent_edit;
use crate::lsp::input_boundaries::InputBoundariesParams;
use crate::lsp::input_boundaries::InputBoundariesResponse;
use crate::lsp::main_loop::LspState;
use crate::lsp::rename;
use crate::lsp::selection_range::convert_selection_range_from_tree_sitter_to_lsp;
use crate::lsp::selection_range::selection_range;
use crate::lsp::signature_help::r_signature_help;
use crate::lsp::signature_help::static_signature_help;
use crate::lsp::signature_help::StaticSignatureHelp;
use crate::lsp::state::WorldState;
use crate::lsp::statement_range::statement_range;
use crate::lsp::statement_range::StatementRangeParams;
use crate::lsp::statement_range::StatementRangeResponse;
use crate::lsp::symbols;
use crate::lsp::traits::url::UriExt;
use crate::r_task;
use crate::r_task::try_r_task;

pub static ARK_VDOC_REQUEST: &str = "ark/internal/virtualDocument";

#[derive(Debug, Eq, PartialEq, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VirtualDocumentParams {
    pub path: String,
}

pub(crate) type VirtualDocumentResponse = String;

// Handlers that do not mutate the world state. They take a sharing reference or
// a clone of the state.

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
pub(crate) fn handle_completion(
    params: CompletionParams,
    state: &WorldState,
    lsp_state: &LspState,
) -> LspResult<Option<CompletionResponse>> {
    let path = params
        .text_document_position
        .text_document
        .uri
        .to_document_path()?;
    let file = state.open_file(&path)?.file();
    let db = state.db();
    let encoding = state.config.position_encoding;

    let position = params.text_document_position.position;
    let point = tree_sitter_point_from_lsp_position(position, file.line_index(db), encoding)?;

    let trigger = params.context.and_then(|ctxt| ctxt.trigger_character);

    let context = DocumentContext::new(
        file.tree_sitter(db),
        file.source_text(db).as_str(),
        file.line_index(db),
        encoding,
        point,
        trigger,
    );
    lsp::log_info!("Completion context: {:#?}", context);

    let (completions, request) = static_completions(&context, file, state)?;

    // If the answer's R libraries, package sources or sidecar facts are still
    // pending, the editor must request completions on the next keystroke rather
    // than reuse a list with missing names or outdated kinds. While libraries
    // resolve, `pkg::` can name a package the db doesn't list yet, and roxygen
    // tags can need a `roxygen2` it doesn't list yet. Their empty answers would
    // otherwise be final. Answers that don't depend on library packages, such
    // as file paths, stay complete during resolution.
    let incomplete = (completions.incomplete_while_libraries_resolve() &&
        lsp_state
            .environment_scheduler
            .is_pending(file.environment(db))) ||
        completions
            .incomplete_until_packages_settle()
            .iter()
            .any(|package| !lsp_state.package_settled(db, &state.config.oak, *package));

    // Without session slots the static items are the whole answer. Skipping
    // the hop also avoids waiting up to `TRY_TASK_START_WITHIN` for an R event
    // loop occupied by another task.
    if !request.has_session_slots() {
        let items = finish(completions, RCompletionReplies::default());
        return Ok(completion_response(items, incomplete));
    }

    // Keep database handles out of the closure. An escaping R error skips
    // destructors, and a leaked Salsa handle would block later writes.
    let Some(replies) = try_r_task(|| r_completions(request)) else {
        // Info level, like the per-source logs, so a kernel log shows why no
        // session source ran
        log::info!("Skipping session completions: R is busy");

        // Every session slot keeps its static fallback. Marked incomplete so
        // the editor requests again on the next keystroke instead of filtering
        // this list, and gets the session items once R is idle.
        let items = finish(completions, RCompletionReplies::default());
        return Ok(completion_response(items, true));
    };

    let items = finish(completions, replies);
    Ok(completion_response(items, incomplete))
}

/// An incomplete answer is a list even when empty, since `null` would end
/// the editor's requests for the current word.
fn completion_response(
    completions: Vec<CompletionItem>,
    is_incomplete: bool,
) -> Option<CompletionResponse> {
    if is_incomplete {
        return Some(CompletionResponse::List(CompletionList {
            is_incomplete,
            items: completions,
        }));
    }

    completion_array(completions)
}

fn completion_array(completions: Vec<CompletionItem>) -> Option<CompletionResponse> {
    if completions.is_empty() {
        None
    } else {
        Some(CompletionResponse::Array(completions))
    }
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_completion_resolve(mut item: CompletionItem) -> LspResult<CompletionItem> {
    // Most item kinds have no documentation to add. Deciding that before the
    // hop avoids waiting up to `TRY_TASK_START_WITHIN` for an R event loop
    // occupied by another task.
    let Some(target) = resolve_target(&item)? else {
        return Ok(item);
    };

    // If R cannot accept the task, return the item unresolved rather than
    // wait for R to become available and block the LSP main loop.
    let Some(result) = try_r_task(|| resolve_completion(&mut item, &target)) else {
        log::trace!("Skipping completion resolve: R is busy");
        return Ok(item);
    };
    result?;
    Ok(item)
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_hover(params: HoverParams, state: &WorldState) -> LspResult<Option<Hover>> {
    let path = params
        .text_document_position_params
        .text_document
        .uri
        .to_document_path()?;
    let file = state.open_file(&path)?.file();
    let db = state.db();
    let encoding = state.config.position_encoding;

    let position = params.text_document_position_params.position;
    let point = tree_sitter_point_from_lsp_position(position, file.line_index(db), encoding)?;

    let context = DocumentContext::new(
        file.tree_sitter(db),
        file.source_text(db).as_str(),
        file.line_index(db),
        encoding,
        point,
        None,
    );

    // Skip rather than block the LSP main loop while R is busy
    let Some(result) = try_r_task(|| r_hover(&context)) else {
        log::trace!("Skipping hover: R is busy");
        return Ok(None);
    };

    // unwrap errors
    let result = unwrap!(result, Err(err) => {
        lsp::log_error!("{err:?}");
        return Ok(None);
    });

    // unwrap empty options
    let result = unwrap!(result, None => {
        return Ok(None);
    });

    // we got a result; use it
    Ok(Some(Hover {
        contents: HoverContents::Markup(result),
        range: None,
    }))
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_signature_help(
    params: SignatureHelpParams,
    state: &WorldState,
) -> LspResult<Option<SignatureHelp>> {
    let path = params
        .text_document_position_params
        .text_document
        .uri
        .to_document_path()?;
    let file = state.open_file(&path)?.file();
    let db = state.db();
    let encoding = state.config.position_encoding;

    let position = params.text_document_position_params.position;
    let point = tree_sitter_point_from_lsp_position(position, file.line_index(db), encoding)?;

    let context = DocumentContext::new(
        file.tree_sitter(db),
        file.source_text(db).as_str(),
        file.line_index(db),
        encoding,
        point,
        None,
    );

    let result = static_signature_help(&context, db, file);
    let result = unwrap!(result, Err(err) => {
        lsp::log_error!("{err:?}");
        return Ok(None);
    });
    let Some(StaticSignatureHelp { help, request }) = result else {
        return Ok(None);
    };

    // While R is busy, return static help without parameter documentation
    // rather than wait for the session. The closure borrows only the request's
    // plain data, so an escaping R error cannot leak a database handle.
    let Some(reply) = try_r_task(|| r_signature_help(&request)) else {
        log::trace!("Skipping session signature help: R is busy");
        return Ok(help);
    };

    match reply {
        Ok(reply) => Ok(reply.help_or(help)),
        Err(err) => {
            lsp::log_error!("{err:?}");
            Ok(help)
        },
    }
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
pub(crate) fn handle_statement_range(
    params: StatementRangeParams,
    state: &WorldState,
) -> LspResult<Option<StatementRangeResponse>> {
    let path = params.text_document.uri.to_document_path()?;
    let file = state.open_file(&path)?.file();
    let db = state.db();
    let encoding = state.config.position_encoding;
    let point =
        tree_sitter_point_from_lsp_position(params.position, file.line_index(db), encoding)?;
    statement_range(db, file, point, encoding)
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_help_topic(
    params: HelpTopicParams,
    state: &WorldState,
) -> LspResult<Option<HelpTopicResponse>> {
    let path = params.text_document.uri.to_document_path()?;
    let file = state.open_file(&path)?.file();
    let db = state.db();
    let encoding = state.config.position_encoding;
    let point =
        tree_sitter_point_from_lsp_position(params.position, file.line_index(db), encoding)?;
    help_topic(db, file, point)
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
    lsp_state: &LspState,
    state: &WorldState,
) -> LspResult<Option<CodeActionResponse>> {
    let path = params.text_document.uri.to_document_path()?;
    let file = state.open_file(&path)?;
    let db = state.db();
    let encoding = state.config.position_encoding;
    let range = tree_sitter_range_from_lsp_range(params.range, file.line_index(db), encoding)?;

    let capabilities = &lsp_state.capabilities;
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

pub(crate) fn handle_virtual_document(
    params: VirtualDocumentParams,
    state: &WorldState,
) -> LspResult<VirtualDocumentResponse> {
    if let Some(contents) = state.virtual_documents.get(&params.path) {
        Ok(contents.clone())
    } else {
        Err(LspError::Anyhow(anyhow!(
            "Can't find virtual document {}",
            params.path
        )))
    }
}

pub(crate) fn handle_input_boundaries(
    params: InputBoundariesParams,
) -> LspResult<InputBoundariesResponse> {
    let boundaries = r_task(|| input_boundaries(&params.text))?;
    Ok(InputBoundariesResponse { boundaries })
}
