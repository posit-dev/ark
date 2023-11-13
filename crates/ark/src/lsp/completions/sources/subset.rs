//
// subset.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::sources::names::completions_from_evaluated_object_names;
use crate::lsp::document_context::DocumentContext;

/// Checks for `[` and `[[` completions
///
/// `$` and `@` are handled elsewhere as they can't be composed with other
/// completions.
pub(super) fn completions_from_subset(
    context: &DocumentContext,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_subset()");

    const ENQUOTE: bool = true;

    let mut node = context.node;

    let mut has_subset = false;

    loop {
        // The `is_named()` is important in case this a case where we found a
        // literal `[` or `[[` operator and need to look up a few parents to
        // find the actual node. We have to do this because `DocumentContext`
        // considers all nodes, not just named ones, and the `[` node and `[`
        // literal operator share the same "kind" name.
        if matches!(node.kind(), "[" | "[[") && node.is_named() {
            has_subset = true;
            break;
        }

        // If we reach a brace list, bail.
        if node.kind() == "{" {
            break;
        }

        // Update the node.
        node = match node.parent() {
            Some(node) => node,
            None => break,
        };
    }

    if !has_subset {
        // Didn't detect anything worth completing in this context,
        // let other sources add their own candidates instead
        return Ok(None);
    }

    let Some(child) = node.child(0) else {
        // There is almost definitely a child here. If there isn't,
        // we "tried" to do subset completions but found nothing.
        return Ok(Some(vec![]));
    };

    let text = child.utf8_text(context.source.as_bytes())?;

    completions_from_evaluated_object_names(&text, ENQUOTE)
}
