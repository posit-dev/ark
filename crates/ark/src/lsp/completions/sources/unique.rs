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

use oak_db::Package;
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

/// The outcome of a unique source that recognized its context. Recognition
/// ends the static search even if the session pass does not run.
#[derive(Debug, PartialEq)]
pub(super) enum Recognized {
    /// Suppresses other sources without requesting session items.
    Empty,
    /// Requests session items, with an empty static fallback.
    Session(UniqueRequest),
    /// Answers without the session.
    Static(UniqueCompletions),
}

/// The answer of the unique source that ended the search.
#[derive(Debug, PartialEq)]
pub(crate) struct UniqueCompletions {
    pub(crate) items: Vec<CompletionItem>,
    /// The library package whose fetched sources can change item kinds or add
    /// namespace names. The answer is incomplete until this package settles.
    pub(crate) incomplete_until_package_settles: Option<Package>,
}

impl UniqueCompletions {
    fn new(items: Vec<CompletionItem>) -> Self {
        Self {
            items,
            incomplete_until_package_settles: None,
        }
    }
}

/// Tries unique sources in priority order until one recognizes the context.
///
/// Session-dependent sources record a request and return `Some` with no items
/// to suppress later sources. Custom callbacks do not end the static search
/// because only the session can determine whether they apply.
pub(crate) fn get_completions(
    completion_context: &CompletionContext,
    request: &mut RCompletionRequest,
) -> anyhow::Result<Option<UniqueCompletions>> {
    log::info!("Getting completions from unique sources");

    // Try to detect a single colon first, which is a special case where we
    // don't provide any completions
    if let Some(completions) = collect_completions(SingleColonSource, completion_context)? {
        return Ok(Some(UniqueCompletions::new(completions)));
    }

    // really about roxygen2 tags
    if let Some(completions) = collect_completions(CommentSource, completion_context)? {
        return Ok(Some(UniqueCompletions::new(completions)));
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
) -> UniqueCompletions {
    log::info!("Unique source `{source_name}` ends the search");

    match recognized {
        Recognized::Empty => UniqueCompletions::new(vec![]),
        Recognized::Session(slot) => {
            request.unique = Some(slot);
            UniqueCompletions::new(vec![])
        },
        Recognized::Static(completions) => completions,
    }
}
