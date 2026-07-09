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
use tower_lsp::lsp_types::CodeActionParams;
use tower_lsp::lsp_types::CodeActionResponse;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionParams;
use tower_lsp::lsp_types::CompletionResponse;
use tower_lsp::lsp_types::DidChangeWatchedFilesRegistrationOptions;
use tower_lsp::lsp_types::DocumentOnTypeFormattingParams;
use tower_lsp::lsp_types::DocumentSymbolParams;
use tower_lsp::lsp_types::DocumentSymbolResponse;
use tower_lsp::lsp_types::FileSystemWatcher;
use tower_lsp::lsp_types::FoldingRange;
use tower_lsp::lsp_types::FoldingRangeParams;
use tower_lsp::lsp_types::GlobPattern;
use tower_lsp::lsp_types::GotoDefinitionParams;
use tower_lsp::lsp_types::GotoDefinitionResponse;
use tower_lsp::lsp_types::Hover;
use tower_lsp::lsp_types::HoverContents;
use tower_lsp::lsp_types::HoverParams;
use tower_lsp::lsp_types::Location;
use tower_lsp::lsp_types::MessageType;
use tower_lsp::lsp_types::PrepareRenameResponse;
use tower_lsp::lsp_types::ReferenceParams;
use tower_lsp::lsp_types::Registration;
use tower_lsp::lsp_types::RenameParams;
use tower_lsp::lsp_types::SelectionRange;
use tower_lsp::lsp_types::SelectionRangeParams;
use tower_lsp::lsp_types::SignatureHelp;
use tower_lsp::lsp_types::SignatureHelpParams;
use tower_lsp::lsp_types::SymbolInformation;
use tower_lsp::lsp_types::TextDocumentPositionParams;
use tower_lsp::lsp_types::TextEdit;
use tower_lsp::lsp_types::WorkspaceEdit;
use tower_lsp::lsp_types::WorkspaceSymbolParams;
use tower_lsp::Client;
use tracing::Instrument;

use crate::analysis::input_boundaries::input_boundaries;
use crate::lsp;
use crate::lsp::backend::LspError;
use crate::lsp::backend::LspResult;
use crate::lsp::code_action::code_actions;
use crate::lsp::completions::provide_completions;
use crate::lsp::completions::resolve_completion;
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
use crate::lsp::open_file::lsp_range_from_tree_sitter_range;
use crate::lsp::open_file::tree_sitter_point_from_lsp_position;
use crate::lsp::open_file::tree_sitter_range_from_lsp_range;
use crate::lsp::rename;
use crate::lsp::selection_range::convert_selection_range_from_tree_sitter_to_lsp;
use crate::lsp::selection_range::selection_range;
use crate::lsp::signature_help::r_signature_help;
use crate::lsp::state::WorldState;
use crate::lsp::statement_range::statement_range;
use crate::lsp::statement_range::StatementRangeParams;
use crate::lsp::statement_range::StatementRangeResponse;
use crate::lsp::symbols;
use crate::r_task;

pub static ARK_VDOC_REQUEST: &str = "ark/internal/virtualDocument";

#[derive(Debug, Eq, PartialEq, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VirtualDocumentParams {
    pub path: String,
}

pub(crate) type VirtualDocumentResponse = String;

// Handlers that do not mutate the world state. They take a sharing reference or
// a clone of the state.

pub(crate) async fn handle_initialized(
    client: &Client,
    lsp_state: &LspState,
) -> anyhow::Result<()> {
    let span = tracing::info_span!("handle_initialized").entered();

    // Register capabilities to the client
    let mut regs: Vec<Registration> = vec![];

    // Watch R files and DESCRIPTION. We get notified on any disk change;
    // the handler skips editor-owned URLs since those are tracked via
    // `textDocument/did*` instead.
    let watchers = vec![
        FileSystemWatcher {
            glob_pattern: GlobPattern::String("**/*.{R,r}".to_string()),
            kind: None,
        },
        FileSystemWatcher {
            glob_pattern: GlobPattern::String("**/DESCRIPTION".to_string()),
            kind: None,
        },
    ];
    regs.push(Registration {
        id: uuid::Uuid::new_v4().to_string(),
        method: String::from("workspace/didChangeWatchedFiles"),
        register_options: Some(
            serde_json::to_value(DidChangeWatchedFilesRegistrationOptions { watchers }).unwrap(),
        ),
    });

    if lsp_state
        .capabilities
        .dynamic_registration_for_did_change_configuration()
    {
        // The `didChangeConfiguration` request instructs the client to send
        // a notification when the tracked settings have changed.
        //
        // Note that some settings, such as editor indentation properties, may be
        // changed by extensions or by the user without changing the actual
        // underlying setting. Unfortunately we don't receive updates in that case.

        for setting in crate::lsp::config::GLOBAL_SETTINGS {
            regs.push(Registration {
                id: uuid::Uuid::new_v4().to_string(),
                method: String::from("workspace/didChangeConfiguration"),
                register_options: Some(serde_json::json!({ "section": setting.key })),
            });
        }
        for setting in crate::lsp::config::DOCUMENT_SETTINGS {
            regs.push(Registration {
                id: uuid::Uuid::new_v4().to_string(),
                method: String::from("workspace/didChangeConfiguration"),
                register_options: Some(serde_json::json!({ "section": setting.key })),
            });
        }
    }

    client
        .register_capability(regs)
        .instrument(span.exit())
        .await?;
    Ok(())
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_symbol(
    params: WorkspaceSymbolParams,
    state: &WorldState,
) -> LspResult<Option<Vec<SymbolInformation>>> {
    symbols::symbols(&params, state).map(Some).or_else(|err| {
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
    symbols::document_symbols(state, &params)
        .map(|res| Some(DocumentSymbolResponse::Nested(res)))
        .or_else(|err| {
            // Missing doc: Why are we not propagating errors to the frontend?
            lsp::log_error!("{err:?}");
            Ok(None)
        })
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_folding_range(
    params: FoldingRangeParams,
    state: &WorldState,
) -> LspResult<Option<Vec<FoldingRange>>> {
    let uri = &params.text_document.uri;
    let file = state.open_file(uri)?.file();
    let db = &state.db;
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
) -> LspResult<Option<CompletionResponse>> {
    let uri = params.text_document_position.text_document.uri;
    let file = state.open_file(&uri)?.file();
    let db = &state.db;
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

    // Snapshot so the closure captures by value. `r_task()` sends the closure
    // across threads, and `&WorldState` isn't `Send` because `OakDatabase`'s
    // salsa storage keeps thread-local query state. `snapshot()` hands the
    // reader a `WorldSnapshot`, so the background thread can query oak but
    // can't call a setter.
    // TODO(oak/completions): We don't really need a snapshot here since
    // completions are serviced from the main loop, it's only needed for the
    // `r_task()`.
    let state = state.snapshot();
    let completions = r_task(move || provide_completions(&context, &state))?;

    if !completions.is_empty() {
        Ok(Some(CompletionResponse::Array(completions)))
    } else {
        Ok(None)
    }
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_completion_resolve(mut item: CompletionItem) -> LspResult<CompletionItem> {
    r_task(|| resolve_completion(&mut item))?;
    Ok(item)
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_hover(params: HoverParams, state: &WorldState) -> LspResult<Option<Hover>> {
    let uri = params.text_document_position_params.text_document.uri;
    let file = state.open_file(&uri)?.file();
    let db = &state.db;
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

    // request hover information
    let result = r_task(|| r_hover(&context));

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
    let uri = params.text_document_position_params.text_document.uri;
    let file = state.open_file(&uri)?.file();
    let db = &state.db;
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

    // request signature help
    let result = r_task(|| r_signature_help(&context));

    // unwrap errors
    let result = unwrap!(result, Err(err) => {
        lsp::log_error!("{err:?}");
        return Ok(None);
    });

    // unwrap empty options
    let result = unwrap!(result, None => {
        return Ok(None);
    });

    Ok(Some(result))
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
    let uri = &params.text_document.uri;
    let file = state.open_file(uri)?.file();
    let db = &state.db;
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
    let uri = &params.text_document.uri;
    let file = state.open_file(uri)?.file();
    let db = &state.db;
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
    let uri = &params.text_document.uri;
    let file = state.open_file(uri)?.file();
    let db = &state.db;
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
    let uri = &ctxt.text_document.uri;
    let open_file = state.open_file(uri)?;
    let encoding = state.config.position_encoding;

    let db = &state.db;
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
    let uri = params.text_document.uri;
    let file = state.open_file(&uri)?;
    let db = &state.db;
    let encoding = state.config.position_encoding;
    let range = tree_sitter_range_from_lsp_range(params.range, file.line_index(db), encoding)?;

    let actions = code_actions(db, file.file(), range, &lsp_state.capabilities);
    let response = actions.into_response(db, file, encoding, &lsp_state.capabilities);

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
