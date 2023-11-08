//
// workspace.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use log::*;
use stdext::*;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;

use crate::lsp::backend::Backend;
use crate::lsp::completions::completion_item::completion_item_from_function;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::indexer;
use crate::lsp::traits::string::StringExt;

pub(super) fn append_workspace_completions(
    backend: &Backend,
    context: &DocumentContext,
    completions: &mut Vec<CompletionItem>,
) -> Result<()> {
    // TODO: Don't provide completions if token is empty in certain contexts
    // (e.g. parameter completions or something like that)
    if matches!(context.node.kind(), "::" | ":::") {
        return Ok(());
    }

    if let Some(parent) = context.node.parent() {
        if matches!(parent.kind(), "::" | ":::") {
            return Ok(());
        }
    }

    if matches!(context.node.kind(), "string") {
        return Ok(());
    }

    let token = if context.node.kind() == "identifier" {
        context.node.utf8_text(context.source.as_bytes())?
    } else {
        ""
    };

    // get entries from the index
    indexer::map(|path, symbol, entry| {
        if !symbol.fuzzy_matches(token) {
            return;
        }

        match &entry.data {
            indexer::IndexEntryData::Function { name, arguments } => {
                let mut completion = unwrap!(completion_item_from_function(name, None, arguments), Err(error) => {
                    error!("{:?}", error);
                    return;
                });

                // add some metadata about where the completion was found
                let mut path = path.to_str().unwrap_or_default();
                let workspace = backend.workspace.lock();
                for folder in &workspace.folders {
                    if let Ok(folder) = folder.to_file_path() {
                        if let Some(folder) = folder.to_str() {
                            if path.starts_with(folder) {
                                path = &path[folder.len() + 1..];
                                break;
                            }
                        }
                    }
                }

                let value = format!(
                    "Defined in `{}` on line {}.",
                    path,
                    entry.range.start.line + 1
                );
                let markup = MarkupContent {
                    kind: MarkupKind::Markdown,
                    value,
                };

                completion.documentation = Some(Documentation::MarkupContent(markup));
                completions.push(completion);
            },

            indexer::IndexEntryData::Section { level: _, title: _ } => {},
        }
    });

    Ok(())
}
