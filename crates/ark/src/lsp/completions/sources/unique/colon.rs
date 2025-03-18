//
// colon.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::builder::CompletionBuilder;
use crate::lsp::completions::sources::CompletionSource;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::rope::RopeExt;

pub struct SingleColonSource;

impl CompletionSource for SingleColonSource {
    fn name(&self) -> &'static str {
        "single_colon"
    }

    fn provide_completions(
        &self,
        builder: &CompletionBuilder,
    ) -> Result<Option<Vec<CompletionItem>>> {
        completions_from_single_colon(builder.context)
    }
}

// Don't provide completions if on a single `:`, which typically precedes
// a `::` or `:::`. It means we don't provide completions for `1:` but we
// accept that.
fn completions_from_single_colon(context: &DocumentContext) -> Result<Option<Vec<CompletionItem>>> {
    if is_single_colon(context) {
        // Return an empty vector to signal that we are done
        Ok(Some(vec![]))
    } else {
        // Let other completions sources contribute
        Ok(None)
    }
}

fn is_single_colon(context: &DocumentContext) -> bool {
    let Ok(slice) = context.document.contents.node_slice(&context.node) else {
        return false;
    };
    slice.eq(":")
}
