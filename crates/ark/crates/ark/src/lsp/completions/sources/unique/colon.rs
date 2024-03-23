//
// colon.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::rope::RopeExt;

// Don't provide completions if on a single `:`, which typically precedes
// a `::` or `:::`. It means we don't provide completions for `1:` but we
// accept that.
pub fn completions_from_single_colon(context: &DocumentContext) -> Option<Vec<CompletionItem>> {
    if is_single_colon(context) {
        // Return an empty vector to signal that we are done
        Some(vec![])
    } else {
        // Let other completions sources contribute
        None
    }
}

fn is_single_colon(context: &DocumentContext) -> bool {
    let Ok(slice) = context.document.contents.node_slice(&context.node) else {
        return false;
    };
    slice.eq(":")
}
