//
// requests.rs
//
// Copyright (C) 2024-2026 Posit Software, PBC. All rights reserved.
//
//

//! Ark's session-backed completion, completion resolve, hover and signature
//! help handlers, plus its custom request handlers.

use std::collections::HashMap;

use anyhow::anyhow;
use stdext::unwrap;
use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::CompletionList;
use tower_lsp_server::ls_types::CompletionParams;
use tower_lsp_server::ls_types::CompletionResponse;
use tower_lsp_server::ls_types::Hover;
use tower_lsp_server::ls_types::HoverContents;
use tower_lsp_server::ls_types::HoverParams;
use tower_lsp_server::ls_types::SignatureHelp;
use tower_lsp_server::ls_types::SignatureHelpParams;

use crate::analysis::input_boundaries::input_boundaries;
use crate::lsp;
use crate::lsp::backend::LspError;
use crate::lsp::backend::LspResult;
use crate::lsp::completions::finish;
use crate::lsp::completions::r_completions;
use crate::lsp::completions::resolve_completion;
use crate::lsp::completions::resolve_target;
use crate::lsp::completions::static_completions;
use crate::lsp::completions::RCompletionReplies;
use crate::lsp::coordinates::tree_sitter_point_from_lsp_position;
use crate::lsp::db::FileLegacyExt;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::handlers::RequestContext;
use crate::lsp::help_topic::help_topic;
use crate::lsp::help_topic::HelpTopicParams;
use crate::lsp::help_topic::HelpTopicResponse;
use crate::lsp::hover::r_hover;
use crate::lsp::input_boundaries::InputBoundariesParams;
use crate::lsp::input_boundaries::InputBoundariesResponse;
use crate::lsp::signature_help::r_signature_help;
use crate::lsp::signature_help::static_signature_help;
use crate::lsp::signature_help::StaticSignatureHelp;
use crate::lsp::state::WorldState;
use crate::lsp::statement_range::statement_range;
use crate::lsp::statement_range::StatementRangeParams;
use crate::lsp::statement_range::StatementRangeResponse;
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

#[tracing::instrument(level = "info", skip_all)]
pub(crate) fn handle_completion(
    params: CompletionParams,
    cx: &RequestContext<'_>,
) -> LspResult<Option<CompletionResponse>> {
    let state = cx.world;
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
        cx.readiness.libraries_pending(file.environment(db))) ||
        completions
            .incomplete_until_packages_settle()
            .iter()
            .any(|package| !cx.readiness.package_settled(*package));

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

pub(crate) fn handle_virtual_document(
    params: VirtualDocumentParams,
    virtual_documents: &HashMap<String, String>,
) -> LspResult<VirtualDocumentResponse> {
    if let Some(contents) = virtual_documents.get(&params.path) {
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
