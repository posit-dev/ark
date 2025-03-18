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

use crate::lsp::completions::builder::CompletionBuilder;
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
        builder: &CompletionBuilder,
    ) -> Result<Option<Vec<CompletionItem>>> {
        let sources: &[&dyn CompletionSource] = &[
            // Try to detect a single colon first, which is a special case where we
            // don't provide any completions
            &SingleColonSource,
            &CommentSource,
            &StringSource,
            &NamespaceSource,
            &CustomSource,
            &DollarSource,
            &AtSource,
        ];
        log::debug!("Getting completions from unique source");

        for source in sources {
            if let Some(completions) = source.provide_completions(builder)? {
                log::debug!("Getting completions from source: {}", source.name());
                return Ok(Some(completions));
            }
        }

        // No unique sources of completions, allow composite sources to run
        Ok(None)
    }
}
