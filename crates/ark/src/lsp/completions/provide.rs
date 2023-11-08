//
// provide.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashSet;

use regex::Regex;
use stdext::*;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;

use crate::lsp::backend::Backend;
use crate::lsp::completions::document::append_document_completions;
use crate::lsp::completions::session::append_session_completions;
use crate::lsp::completions::workspace::append_workspace_completions;
use crate::lsp::document_context::DocumentContext;
use crate::r_task;

// Entry point for completions
pub fn provide_completions(backend: &Backend, context: &DocumentContext) -> Vec<CompletionItem> {
    // Start building completions
    let mut completions: Vec<CompletionItem> = vec![];

    // Don't provide completions if on a single `:`, which typically precedes
    // a `::` or `:::`. It means we don't provide completions for `1:` but we
    // accept that.
    if is_single_colon(context) {
        return completions;
    }

    // TODO: These probably shouldn't be separate methods, because we might get
    // the same completion from multiple sources, e.g.
    //
    // - A completion for a function 'foo' defined in the current document,
    // - A completion for a function 'foo' defined in the workspace,
    // - A variable called 'foo' defined in the current R session.
    //
    // Really, what's relevant is which of the above should be considered
    // 'visible' to the user.

    // Add session completions
    let result = r_task(|| unsafe { append_session_completions(context, &mut completions) });
    if let Err(err) = result {
        log::error!("{err:?}");
    }

    // Add context-relevant completions
    let result = append_document_completions(context, &mut completions);
    if let Err(err) = result {
        log::error!("{err:?}");
    }

    // Add workspace completions
    let result = append_workspace_completions(backend, context, &mut completions);
    if let Err(err) = result {
        log::error!("{err:?}");
    }

    // Remove duplicates
    let mut uniques = HashSet::new();
    completions.retain(|x| uniques.insert(x.label.clone()));

    // Remove completions that start with `.` unless the user explicitly requested them
    // TODO: This ends up removing function argument completions if they start with `.`,
    // which is pretty common
    let user_requested_dot = context
        .node
        .utf8_text(context.source.as_bytes())
        .and_then(|x| Ok(x.starts_with(".")))
        .unwrap_or(false);

    if !user_requested_dot {
        completions.retain(|x| !x.label.starts_with("."));
    }

    // sort completions by providing custom 'sort' text to be used when
    // ordering completion results. we use some placeholders at the front
    // to 'bin' different completion types differently; e.g. we place parameter
    // completions at the front, and completions starting with non-word
    // characters at the end (e.g. completions starting with `.`)
    let pattern = Regex::new(r"^\w").unwrap();
    for item in &mut completions {
        case! {

            item.kind == Some(CompletionItemKind::FIELD) => {
                item.sort_text = Some(join!["1", item.label]);
            }

            item.kind == Some(CompletionItemKind::VARIABLE) => {
                item.sort_text = Some(join!["2", item.label]);
            }

            pattern.is_match(&item.label) => {
                item.sort_text = Some(join!["3", item.label]);
            }

            => {
                item.sort_text = Some(join!["4", item.label]);
            }

        }
    }

    completions
}

fn is_single_colon(context: &DocumentContext) -> bool {
    context
        .node
        .utf8_text(context.source.as_bytes())
        .unwrap_or("")
        .eq(":")
}
