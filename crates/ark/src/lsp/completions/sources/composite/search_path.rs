//
// search_path.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::request::SearchPathRequest;
use crate::lsp::completions::sources::utils::user_requested_dot;

pub(super) fn search_path_request(
    context: &CompletionContext,
) -> anyhow::Result<SearchPathRequest> {
    Ok(SearchPathRequest {
        function_context: context.function_context()?.clone(),
        dot_prefixed: user_requested_dot(context.document_context),
    })
}
