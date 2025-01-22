//
// provide.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_utils::check_for_function_value;
use crate::lsp::completions::completion_utils::check_for_help;
use crate::lsp::completions::completion_utils::gather_completion_context;
use crate::lsp::completions::sources::completions_from_composite_sources;
use crate::lsp::completions::sources::completions_from_unique_sources;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::state::WorldState;

// Entry point for completions.
// Must be within an `r_task()`.
pub(crate) fn provide_completions(
    context: &DocumentContext,
    state: &WorldState,
) -> Result<Vec<CompletionItem>> {
    log::info!("provide_completions()");

    let node_context = gather_completion_context(context);
    let no_trailing_parens =
        check_for_function_value(context, &node_context) || check_for_help(&node_context);

    if let Some(completions) = completions_from_unique_sources(context, no_trailing_parens)? {
        return Ok(completions);
    };

    // At this point we aren't in a "unique" completion case, so just return a
    // set of reasonable completions based on loaded packages, the open
    // document, the current workspace, and any call related arguments
    completions_from_composite_sources(context, state, no_trailing_parens)
}
