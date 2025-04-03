//
// sources.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

mod common;
pub(crate) mod composite;
pub(crate) mod unique;
mod utils;

use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_context::CompletionContext;

/// Interface for any source we consult for completions
pub trait CompletionSource {
    /// Name of this source for logging/debugging
    fn name(&self) -> &'static str;

    fn provide_completions(
        &self,
        completion_context: &CompletionContext,
    ) -> anyhow::Result<Option<Vec<CompletionItem>>>;
}

pub fn collect_completions<S>(
    source: S,
    completion_context: &CompletionContext,
) -> anyhow::Result<Option<Vec<CompletionItem>>>
where
    S: CompletionSource,
{
    let source_name = source.name();
    log::info!("Trying completions from source: {}", source_name);

    if let Some(completions) = source.provide_completions(completion_context)? {
        log::info!(
            "Found {} completions from source: {}",
            completions.len(),
            source_name
        );
        Ok(Some(completions))
    } else {
        Ok(None)
    }
}
