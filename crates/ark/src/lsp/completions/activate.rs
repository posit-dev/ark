//
// activate.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use stdext::*;
use tower_lsp::lsp_types::CompletionParams;

use crate::lsp::completions::types::CompletionContext;

pub fn can_provide_completions(
    context: &CompletionContext,
    params: &CompletionParams,
) -> Result<bool> {
    // If this completion was triggered by the user typing a ':', then only
    // provide completions if the current node already has '::' or ':::'.
    if let Some(ref completion_context) = params.context {
        if let Some(ref trigger_character) = completion_context.trigger_character {
            if trigger_character == ":" {
                let text = context.node.utf8_text(context.source.as_bytes())?;
                if !matches!(text, "::" | ":::") {
                    return false.ok();
                }
            }
        }
    }

    true.ok()
}
