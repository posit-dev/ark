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

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_context::CompletionContext;
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
    ) -> Result<Option<Vec<CompletionItem>>> {
        log::info!("Getting completions from unique sources");

        let sources: &[&dyn CompletionSource] = &[
            // Try to detect a single colon first, which is a special case where we
            // don't provide any completions
            &SingleColonSource,
            &CommentSource,   // really about roxygen2 tags
            &StringSource,    // could be a file path
            &NamespaceSource, // pkg::xxx or pkg::::xxx
            &CustomSource,    // custom completions for, eg, options or env vars
            &DollarSource,    // as in foo$bar
            &AtSource,        // as in foo@bar
        ];

        for source in sources {
            let source_name = source.name();
            log::debug!("Trying completions from source: {}", source_name);

            if let Some(completions) = source.provide_completions(completion_context)? {
                log::info!(
                    "Found {} completions from source: {}",
                    completions.len(),
                    source_name
                );
                return Ok(Some(completions));
            }
        }

        log::debug!("No unique source provided completions");
        Ok(None)
    }
}
