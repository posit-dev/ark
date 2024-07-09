//
// unique.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

mod colon;
mod comment;
mod custom;
mod extractor;
mod file_path;
mod namespace;

// TODO: string can be either a unique case (e.g. file path) or a composite case
// (e.g. a variable name in `[` operator). To handle the both case, use this in
// the code for composite cases because otherwise it returns early and never
// visits the latter case.
pub(crate) mod string;

use anyhow::Result;
use colon::completions_from_single_colon;
use comment::completions_from_comment;
use custom::completions_from_custom_source;
use extractor::completions_from_at;
use extractor::completions_from_dollar;
use namespace::completions_from_namespace;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::document_context::DocumentContext;

pub fn completions_from_unique_sources(
    context: &DocumentContext,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_unique_sources()");

    // Try to detect a single colon first, which is a special case where we
    // don't provide any completions
    if let Some(completions) = completions_from_single_colon(context) {
        return Ok(Some(completions));
    }

    // Try comment / roxygen2 completions
    if let Some(completions) = completions_from_comment(context)? {
        return Ok(Some(completions));
    }

    // Try `package::prefix` (or `:::`) namespace completions
    if let Some(completions) = completions_from_namespace(context)? {
        return Ok(Some(completions));
    }

    // Try specialized custom completions
    // (Should be before more general ast / call completions)
    if let Some(completions) = completions_from_custom_source(context)? {
        return Ok(Some(completions));
    }

    // Try `$` completions
    if let Some(completions) = completions_from_dollar(context)? {
        return Ok(Some(completions));
    }

    // Try `@` completions
    if let Some(completions) = completions_from_at(context)? {
        return Ok(Some(completions));
    }

    // No unique sources of completions, allow composite sources to run
    Ok(None)
}
