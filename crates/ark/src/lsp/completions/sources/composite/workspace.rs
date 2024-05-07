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
use crate::lsp::completions::sources::utils::filter_out_dot_prefixes;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::indexer;
use crate::lsp::traits::rope::RopeExt;
use crate::lsp::traits::string::StringExt;
use crate::treesitter::NodeTypeExt;

pub(super) fn completions_from_workspace(
    backend: &Backend,
    context: &DocumentContext,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_workspace()");

    let node = context.node;

    if node.is_namespace_operator() {
        log::error!("Should have already been handled by namespace completions source");
        return Ok(None);
    }
    if let Some(parent) = node.parent() {
        if parent.is_namespace_operator() {
            log::error!("Should have already been handled by namespace completions source");
            return Ok(None);
        }
    }

    if node.is_string() {
        log::error!("Should have already been handled by file path completions source");
        return Ok(None);
    }

    let mut completions = vec![];

    let token = if node.is_identifier() {
        context.document.contents.node_slice(&node)?.to_string()
    } else {
        "".to_string()
    };
    let token = token.as_str();

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
                let workspace = backend.state.workspace.lock();
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

    // Assume that even if they are in the workspace, we still don't want
    // to include them without explicit user request.
    // In particular, public modules in Positron
    filter_out_dot_prefixes(context, &mut completions);

    Ok(Some(completions))
}
