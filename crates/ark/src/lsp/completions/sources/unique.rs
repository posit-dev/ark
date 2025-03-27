//
// unique.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

mod colon;
mod comment;
mod custom;
mod extractor;
mod file_path;
mod namespace;
mod string;
mod subset;

use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::sources::collect_completions;
use crate::lsp::completions::sources::unique::colon::SingleColonSource;
use crate::lsp::completions::sources::unique::comment::CommentSource;
use crate::lsp::completions::sources::unique::custom::CustomSource;
use crate::lsp::completions::sources::unique::extractor::AtSource;
use crate::lsp::completions::sources::unique::extractor::DollarSource;
use crate::lsp::completions::sources::unique::namespace::NamespaceSource;
use crate::lsp::completions::sources::unique::string::StringSource;
use crate::lsp::completions::sources::CompletionSource;

/// Aggregator for unique completion sources
/// This source tries each unique source in order and returns the first time
/// a source returns completions (with the caveat that single colon completions
/// are special).
pub struct UniqueCompletionsSource;

impl CompletionSource for UniqueCompletionsSource {
    fn name(&self) -> &'static str {
        "unique_sources"
    }

    fn provide_completions(
        &self,
        completion_context: &CompletionContext,
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
        if let Some(completions) = collect_completions(StringSource, completion_context)? {
            return Ok(Some(completions));
        }

        // pkg::xxx or pkg:::xxx
        if let Some(completions) = collect_completions(NamespaceSource, completion_context)? {
            return Ok(Some(completions));
        }

        // custom completions for, e.g., options or env vars
        if let Some(completions) = collect_completions(CustomSource, completion_context)? {
            return Ok(Some(completions));
        }

        // as in foo$bar
        if let Some(completions) = collect_completions(DollarSource, completion_context)? {
            return Ok(Some(completions));
        }

        // as in foo@bar
        if let Some(completions) = collect_completions(AtSource, completion_context)? {
            return Ok(Some(completions));
        }

        log::trace!("No unique source provided completions");
        Ok(None)
    }
}
