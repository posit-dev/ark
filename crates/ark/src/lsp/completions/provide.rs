//
// provide.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::backend::Backend;
use crate::lsp::completions::sources::completions_from_at;
use crate::lsp::completions::sources::completions_from_comment;
use crate::lsp::completions::sources::completions_from_composite_sources;
use crate::lsp::completions::sources::completions_from_custom_source;
use crate::lsp::completions::sources::completions_from_dollar;
use crate::lsp::completions::sources::completions_from_file_path;
use crate::lsp::completions::sources::completions_from_namespace;
use crate::lsp::document_context::DocumentContext;

// Entry point for completions.
// Must be within an `r_task()`.
pub fn provide_completions(
    backend: &Backend,
    context: &DocumentContext,
) -> Result<Vec<CompletionItem>> {
    log::info!("provide_completions()");

    // Don't provide completions if on a single `:`, which typically precedes
    // a `::` or `:::`. It means we don't provide completions for `1:` but we
    // accept that.
    if is_single_colon(context) {
        return Ok(vec![]);
    }

    // Try comment / roxygen2 completions
    if let Some(completions) = completions_from_comment(context)? {
        return Ok(completions);
    }

    // Try file completions
    if let Some(completions) = completions_from_file_path(context)? {
        return Ok(completions);
    }

    // Try `package::prefix` (or `:::`) namespace completions
    if let Some(completions) = completions_from_namespace(context)? {
        return Ok(completions);
    }

    // Try specialized custom completions
    // (Should be before more general ast / call completions)
    if let Some(completions) = completions_from_custom_source(context)? {
        return Ok(completions);
    }

    // Try `$` completions
    if let Some(completions) = completions_from_dollar(context)? {
        return Ok(completions);
    }

    // Try `@` completions
    if let Some(completions) = completions_from_at(context)? {
        return Ok(completions);
    }

    // At this point we aren't in a "special" completion case, so just return a
    // set of reasonable completions based on loaded packages, the open
    // document, the current workspace, and any call related arguments
    completions_from_composite_sources(backend, context)
}

fn is_single_colon(context: &DocumentContext) -> bool {
    context
        .node
        .utf8_text(context.source.as_bytes())
        .unwrap_or("")
        .eq(":")
}
