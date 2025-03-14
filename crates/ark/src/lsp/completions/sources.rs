//
// sources.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

mod common;
pub(crate) mod composite;
pub mod unique;
mod utils;

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::builder::CompletionBuilder;

/// Interface for any source we consult for completions
pub trait CompletionSource {
    /// Name of this source for logging/debugging
    /// Not used (yet?) but seems like a good idea
    #[allow(dead_code)]
    fn name(&self) -> &'static str;

    fn provide_completions(builder: &CompletionBuilder) -> Result<Option<Vec<CompletionItem>>>;
}
