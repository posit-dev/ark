//
// colon.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::sources::CompletionSource;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::node::NodeExt;

pub(super) struct SingleColonSource;

impl CompletionSource for SingleColonSource {
    fn name(&self) -> &'static str {
        "single_colon"
    }

    fn provide_completions(
        &self,
        completion_context: &CompletionContext,
    ) -> anyhow::Result<Option<Vec<CompletionItem>>> {
        completions_from_single_colon(completion_context.document_context)
    }
}

// Don't provide completions if on a single `:`, which typically precedes
// a `::` or `:::`. It means we don't provide completions for `1:` but we
// accept that.
fn completions_from_single_colon(
    context: &DocumentContext,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    if is_single_colon(context) {
        // Return an empty vector to signal that we are done
        Ok(Some(vec![]))
    } else {
        // Let other completions sources contribute
        Ok(None)
    }
}

fn is_single_colon(context: &DocumentContext) -> bool {
    let Ok(text) = context.node.node_as_str(&context.document.contents) else {
        return false;
    };
    text.eq(":")
}
