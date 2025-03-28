//
// provide.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::sources::composite;
use crate::lsp::completions::sources::unique;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::state::WorldState;

// Entry point for completions.
// Must be within an `r_task()`.
pub(crate) fn provide_completions(
    document_context: &DocumentContext,
    state: &WorldState,
) -> anyhow::Result<Vec<CompletionItem>> {
    log::info!("provide_completions()");

    let completion_context = CompletionContext::new(document_context, state);

    // Try unique sources first
    if let Some(completions) = unique::get_completions(&completion_context)? {
        return Ok(completions);
    }

    // At this point we aren't in a "unique" completion case, so just return a
    // set of reasonable completions from composite sources
    Ok(composite::get_completions(&completion_context)?.unwrap_or_default())
}
