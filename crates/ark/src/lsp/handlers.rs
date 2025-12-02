//
// handlers.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use anyhow::anyhow;
use serde_json::Value;
use stdext::result::ResultExt;
use stdext::unwrap;
use stdext::unwrap::IntoResult;
use tower_lsp::lsp_types::CodeActionParams;
use tower_lsp::lsp_types::CodeActionResponse;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionParams;
use tower_lsp::lsp_types::CompletionResponse;
use tower_lsp::lsp_types::DocumentOnTypeFormattingParams;
use tower_lsp::lsp_types::DocumentSymbolParams;
use tower_lsp::lsp_types::DocumentSymbolResponse;
use tower_lsp::lsp_types::FoldingRange;
use tower_lsp::lsp_types::FoldingRangeParams;
use tower_lsp::lsp_types::GotoDefinitionParams;
use tower_lsp::lsp_types::GotoDefinitionResponse;
use tower_lsp::lsp_types::Hover;
use tower_lsp::lsp_types::HoverContents;
use tower_lsp::lsp_types::HoverParams;
use tower_lsp::lsp_types::Location;
use tower_lsp::lsp_types::MessageType;
use tower_lsp::lsp_types::ReferenceParams;
use tower_lsp::lsp_types::Registration;
use tower_lsp::lsp_types::SelectionRange;
use tower_lsp::lsp_types::SelectionRangeParams;
use tower_lsp::lsp_types::SignatureHelp;
use tower_lsp::lsp_types::SignatureHelpParams;
use tower_lsp::lsp_types::SymbolInformation;
use tower_lsp::lsp_types::TextEdit;
use tower_lsp::lsp_types::WorkspaceEdit;
use tower_lsp::lsp_types::WorkspaceSymbolParams;
use tower_lsp::Client;
use tracing::Instrument;
use tree_sitter::Point;

use crate::analysis::input_boundaries::input_boundaries;
use crate::lsp;
use crate::lsp::code_action::code_actions;
use crate::lsp::completions::provide_completions;
use crate::lsp::completions::resolve_completion;
use crate::lsp::definitions::goto_definition;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::folding_range::folding_range;
use crate::lsp::help_topic::help_topic;
use crate::lsp::help_topic::HelpTopicParams;
use crate::lsp::help_topic::HelpTopicResponse;
use crate::lsp::hover::r_hover;
use crate::lsp::indent::indent_edit;
use crate::lsp::input_boundaries::InputBoundariesParams;
use crate::lsp::input_boundaries::InputBoundariesResponse;
use crate::lsp::main_loop::LspState;
use crate::lsp::references::find_references;
use crate::lsp::selection_range::convert_selection_range_from_tree_sitter_to_lsp;
use crate::lsp::selection_range::selection_range;
use crate::lsp::signature_help::r_signature_help;
use crate::lsp::state::WorldState;
use crate::lsp::statement_range::statement_range;
use crate::lsp::statement_range::StatementRangeParams;
use crate::lsp::statement_range::StatementRangeResponse;
use crate::lsp::symbols;
use crate::r_task;

pub static ARK_VDOC_REQUEST: &'static str = "ark/internal/virtualDocument";

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
) -> anyhow::Result<Option<Vec<SymbolInformation>>> {
    symbols::symbols(&params, state)
        .map(|res| Some(res))
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
) -> anyhow::Result<Option<DocumentSymbolResponse>> {
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
) -> anyhow::Result<Option<Vec<FoldingRange>>> {
    let uri = params.text_document.uri;
    let document = state.documents.get(&uri).into_result()?;
    match folding_range(document) {
        Ok(foldings) => Ok(Some(foldings)),
        Err(err) => {
            lsp::log_error!("{err:?}");
            Ok(None)
        },
    }
}

pub(crate) async fn handle_execute_command(client: &Client) -> anyhow::Result<Option<Value>> {
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
) -> anyhow::Result<Option<CompletionResponse>> {
    // Get reference to document.
    let uri = params.text_document_position.text_document.uri;
    let document = state.get_document(&uri)?;

    let position = params.text_document_position.position;
    let point = document.tree_sitter_point_from_lsp_position(position)?;

    let trigger = params.context.and_then(|ctxt| ctxt.trigger_character);

    // Build the document context.
    let context = DocumentContext::new(&document, point, trigger);
    lsp::log_info!("Completion context: {:#?}", context);

    let completions = r_task(|| provide_completions(&context, state))?;

    if !completions.is_empty() {
        Ok(Some(CompletionResponse::Array(completions)))
    } else {
        Ok(None)
    }
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_completion_resolve(
    mut item: CompletionItem,
) -> anyhow::Result<CompletionItem> {
    r_task(|| resolve_completion(&mut item))?;
    Ok(item)
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_hover(
    params: HoverParams,
    state: &WorldState,
) -> anyhow::Result<Option<Hover>> {
    let uri = params.text_document_position_params.text_document.uri;
    let document = state.get_document(&uri)?;

    let position = params.text_document_position_params.position;
    let point = document.tree_sitter_point_from_lsp_position(position)?;

    // build document context
    let context = DocumentContext::new(&document, point, None);

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
) -> anyhow::Result<Option<SignatureHelp>> {
    let uri = params.text_document_position_params.text_document.uri;
    let document = state.get_document(&uri)?;

    let position = params.text_document_position_params.position;
    let point = document.tree_sitter_point_from_lsp_position(position)?;

    let context = DocumentContext::new(&document, point, None);

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
) -> anyhow::Result<Option<GotoDefinitionResponse>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let document = state.get_document(uri)?;
    Ok(goto_definition(&document, params).log_err().flatten())
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_selection_range(
    params: SelectionRangeParams,
    state: &WorldState,
) -> anyhow::Result<Option<Vec<SelectionRange>>> {
    let document = state.get_document(&params.text_document.uri)?;

    // Get tree-sitter points to return selection ranges for
    let points: anyhow::Result<Vec<Point>> = params
        .positions
        .into_iter()
        .map(|position| document.tree_sitter_point_from_lsp_position(position))
        .collect();
    let points = points?;

    let Some(selections) = selection_range(&document.ast, points) else {
        return Ok(None);
    };

    // Convert tree-sitter points to LSP positions everywhere
    let selections = selections
        .into_iter()
        .map(|selection| convert_selection_range_from_tree_sitter_to_lsp(selection, &document))
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(Some(selections))
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_references(
    params: ReferenceParams,
    state: &WorldState,
) -> anyhow::Result<Option<Vec<Location>>> {
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
pub(crate) fn handle_statement_range(
    params: StatementRangeParams,
    state: &WorldState,
) -> anyhow::Result<Option<StatementRangeResponse>> {
    let document = state.get_document(&params.text_document.uri)?;
    let point = document.tree_sitter_point_from_lsp_position(params.position)?;
    statement_range(document, point)
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_help_topic(
    params: HelpTopicParams,
    state: &WorldState,
) -> anyhow::Result<Option<HelpTopicResponse>> {
    let document = state.get_document(&params.text_document.uri)?;
    let point = document.tree_sitter_point_from_lsp_position(params.position)?;
    help_topic(point, &document)
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_indent(
    params: DocumentOnTypeFormattingParams,
    state: &WorldState,
) -> anyhow::Result<Option<Vec<TextEdit>>> {
    let ctxt = params.text_document_position;
    let doc = state.get_document(&ctxt.text_document.uri)?;
    let point = doc.tree_sitter_point_from_lsp_position(ctxt.position)?;

    indent_edit(doc, point.row)
}

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_code_action(
    params: CodeActionParams,
    lsp_state: &LspState,
    state: &WorldState,
) -> anyhow::Result<Option<CodeActionResponse>> {
    let uri = params.text_document.uri;
    let doc = state.get_document(&uri)?;
    let range = doc.tree_sitter_range_from_lsp_range(params.range)?;

    let code_actions = code_actions(&uri, doc, range, &lsp_state.capabilities);

    if code_actions.is_empty() {
        Ok(None)
    } else {
        Ok(Some(code_actions))
    }
}

pub(crate) fn handle_virtual_document(
    params: VirtualDocumentParams,
    state: &WorldState,
) -> anyhow::Result<VirtualDocumentResponse> {
    if let Some(contents) = state.virtual_documents.get(&params.path) {
        Ok(contents.clone())
    } else {
        Err(anyhow!("Can't find virtual document {}", params.path))
    }
}

pub(crate) fn handle_input_boundaries(
    params: InputBoundariesParams,
) -> anyhow::Result<InputBoundariesResponse> {
    let boundaries = r_task(|| input_boundaries(&params.text))?;
    Ok(InputBoundariesResponse { boundaries })
}
