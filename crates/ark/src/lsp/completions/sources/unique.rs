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
use crate::lsp::completions::sources::unique::colon::completions_from_single_colon;
use crate::lsp::completions::sources::unique::comment::completions_from_comment;
use crate::lsp::completions::sources::unique::custom::completions_from_custom_source;
use crate::lsp::completions::sources::unique::extractor::completions_from_at;
use crate::lsp::completions::sources::unique::extractor::completions_from_dollar;
use crate::lsp::completions::sources::unique::namespace::NamespaceSource;
use crate::lsp::completions::sources::unique::string::completions_from_string;
use crate::lsp::completions::sources::CompletionSource;

/// Aggregator for unique completion sources
/// This source tries each unique source in order and returns the first set of
/// completions that match.
pub struct UniqueCompletionsSource;

impl CompletionSource for UniqueCompletionsSource {
    fn name(&self) -> &'static str {
        "unique_sources"
    }

    fn provide_completions(builder: &CompletionBuilder) -> Result<Option<Vec<CompletionItem>>> {
        // Try to detect a single colon first, which is a special case where we
        // don't provide any completions
        if let Some(completions) = completions_from_single_colon(builder.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_comment(builder.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_string(builder.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = NamespaceSource::provide_completions(builder)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_custom_source(builder.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_dollar(builder.context)? {
            return Ok(Some(completions));
        }

        if let Some(completions) = completions_from_at(builder.context)? {
            return Ok(Some(completions));
        }

        // No unique sources of completions, allow composite sources to run
        Ok(None)
    }
}
