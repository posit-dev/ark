//
// provide.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::builder::CompletionBuilder;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::state::WorldState;

// Entry point for completions.
// Must be within an `r_task()`.
pub(crate) fn provide_completions(
    context: &DocumentContext,
    state: &WorldState,
) -> Result<Vec<CompletionItem>> {
    log::info!("provide_completions()");

    CompletionBuilder::new(context, state).build()
}
