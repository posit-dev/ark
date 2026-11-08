//
// unique.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

mod colon;
mod comment;
mod custom;
mod extractor;
mod namespace;
mod string;
mod subset;

use tower_lsp_server::ls_types::CompletionItem;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::request::RCompletionRequest;
use crate::lsp::completions::request::UniqueRequest;
use crate::lsp::completions::sources::collect_completions;
use crate::lsp::completions::sources::unique::colon::SingleColonSource;
use crate::lsp::completions::sources::unique::comment::CommentSource;
use crate::lsp::completions::sources::unique::custom::custom_request;
use crate::lsp::completions::sources::unique::extractor::recognize_at;
use crate::lsp::completions::sources::unique::extractor::recognize_dollar;
use crate::lsp::completions::sources::unique::namespace::recognize_namespace;
use crate::lsp::completions::sources::unique::string::recognize_string;

/// A context recognized by a session-dependent unique source. Recognition
/// ends the static search even if the session pass does not run.
#[derive(Debug, PartialEq)]
pub(super) enum Recognized {
    /// Suppresses other sources without requesting session items.
    Empty,
    /// Requests session items, with an empty static fallback.
    Session(UniqueRequest),
}

/// Tries unique sources in priority order until one recognizes the context.
///
/// Session-dependent sources record a request and return `Some` with no items
/// to suppress later sources. Custom callbacks do not end the static search
/// because only the session can determine whether they apply.
pub(crate) fn get_completions(
    completion_context: &CompletionContext,
    request: &mut RCompletionRequest,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    log::info!("Getting completions from unique sources");

    // Try to detect a single colon first, which is a special case where we
    // don't provide any completions
    if let Some(completions) = collect_completions(SingleColonSource, completion_context)? {
        return Ok(Some(completions));
    }

    // really about roxygen2 tags
    if let Some(completions) = collect_completions(CommentSource, completion_context)? {
        return Ok(Some(completions));
    }

    // could be a file path
    if let Some(recognized) = recognize_string(completion_context.document_context)? {
        return Ok(Some(end_search("string", recognized, request)));
    }

    // pkg::xxx or pkg:::xxx
    if let Some(recognized) = recognize_namespace(completion_context)? {
        return Ok(Some(end_search("namespace", recognized, request)));
    }

    // Only the session can determine whether a custom callback applies.
    // Continue the search to provide a fallback if it declines.
    request.custom = custom_request(completion_context)?;

    // as in foo$bar
    if let Some(recognized) = recognize_dollar(completion_context.document_context)? {
        return Ok(Some(end_search("dollar", recognized, request)));
    }

    // as in foo@bar
    if let Some(recognized) = recognize_at(completion_context.document_context)? {
        return Ok(Some(end_search("at", recognized, request)));
    }

    log::info!("No unique source provided completions");
    Ok(None)
}

fn end_search(
    source_name: &str,
    recognized: Recognized,
    request: &mut RCompletionRequest,
) -> Vec<CompletionItem> {
    log::info!("Unique source `{source_name}` ends the search");

    if let Recognized::Session(slot) = recognized {
        request.unique = Some(slot);
    }

    vec![]
}
