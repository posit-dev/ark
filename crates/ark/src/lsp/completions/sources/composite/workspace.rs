//
// workspace.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::Documentation;
use tower_lsp::lsp_types::MarkupContent;
use tower_lsp::lsp_types::MarkupKind;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::completion_item::completion_item_from_function;
use crate::lsp::completions::completion_item::completion_item_from_variable;
use crate::lsp::completions::sources::utils::filter_out_dot_prefixes;
use crate::lsp::completions::sources::CompletionSource;
use crate::lsp::indexer;
use crate::lsp::traits::node::NodeExt;
use crate::lsp::traits::string::StringExt;
use crate::treesitter::node_in_string;
use crate::treesitter::NodeTypeExt;

pub(super) struct WorkspaceSource;

impl CompletionSource for WorkspaceSource {
    fn name(&self) -> &'static str {
        "workspace"
    }

    fn provide_completions(
        &self,
        completion_context: &CompletionContext,
    ) -> anyhow::Result<Option<Vec<CompletionItem>>> {
        completions_from_workspace(completion_context)
    }
}

fn completions_from_workspace(
    completion_context: &CompletionContext,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    let context = completion_context.document_context;
    let state = completion_context.state;
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

    if node_in_string(&node) {
        log::error!("Should have already been handled by string completions source");
        return Ok(None);
    }

    let mut completions = vec![];

    let token = if node.is_identifier() {
        node.node_as_str(&context.document.contents)?.to_string()
    } else {
        "".to_string()
    };
    let token = token.as_str();

    // get entries from the index
    indexer::map(|uri, symbol, entry| {
        if !symbol.fuzzy_matches(token) {
            return;
        }

        match &entry.data {
            indexer::IndexEntryData::Function { name, .. } => {
                let fun_context = match completion_context.function_context() {
                    Ok(fun_context) => fun_context,
                    Err(err) => {
                        log::error!("{:?}", err);
                        return;
                    },
                };

                let mut completion = match completion_item_from_function(name, None, fun_context) {
                    Ok(completion) => completion,
                    Err(err) => {
                        log::error!("{:?}", err);
                        return;
                    },
                };

                // Add some metadata about where the completion was found
                let mut path = uri.as_str().to_owned();

                if uri.scheme() == "file" {
                    if let Ok(file_path) = uri.to_file_path() {
                        for folder in &state.workspace.folders {
                            let Ok(folder_path) = folder.to_file_path() else {
                                continue;
                            };
                            if let Ok(relative_path) = file_path.strip_prefix(&folder_path) {
                                path = relative_path.to_string_lossy().to_string();
                                break;
                            }
                        }
                    };
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
            indexer::IndexEntryData::Variable { name } => {
                let completion = match completion_item_from_variable(name) {
                    Ok(item) => item,
                    Err(err) => {
                        log::error!("{err:?}");
                        return;
                    },
                };
                completions.push(completion);
            },

            // Methods are currently only indexed for workspace symbols
            indexer::IndexEntryData::Method { .. } => {},
        }
    });

    // Assume that even if they are in the workspace, we still don't want
    // to include them without explicit user request.
    // In particular, public modules in Positron
    filter_out_dot_prefixes(context, &mut completions);

    Ok(Some(completions))
}
