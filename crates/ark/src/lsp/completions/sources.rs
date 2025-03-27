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

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_context::CompletionContext;

/// Interface for any source we consult for completions
pub trait CompletionSource {
    /// Name of this source for logging/debugging
    fn name(&self) -> &'static str;

    fn provide_completions(
        &self,
        completion_context: &CompletionContext,
    ) -> Result<Option<Vec<CompletionItem>>>;
}
