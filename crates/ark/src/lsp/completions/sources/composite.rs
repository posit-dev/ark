//
// composite.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

mod call;
mod document;
mod keyword;
pub(crate) mod pipe;
mod search_path;
mod snippets;
mod subset;
mod workspace;

use std::collections::HashMap;

use anyhow::Result;
pub use pipe::find_pipe_root;
use stdext::*;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;

use crate::lsp::completions::builder::CompletionBuilder;
use crate::lsp::completions::sources::CompletionSource;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

/// Aggregator for composite completion sources
/// Does deduplication and sorting also
pub struct CompositeCompletionsSource;

impl CompletionSource for CompositeCompletionsSource {
    fn name(&self) -> &'static str {
        "composite_sources"
    }

    fn provide_completions(
        &self,
        builder: &CompletionBuilder,
    ) -> Result<Option<Vec<CompletionItem>>> {
        let mut completions = HashMap::new();

        // Gather completions into our local collection
        self.gather_completions(builder, &mut completions)?;

        // Turn local collection into a sorted list of plain CompletionItems
        Ok(Some(self.get_completions(&completions)))
    }
}

// Locally useful data structures for tracking completions and their source(s)
#[derive(Hash, Eq, PartialEq, Clone)]
struct CompletionKey {
    label: String,
    insert_text: Option<String>,
}

impl From<&CompletionItem> for CompletionKey {
    fn from(item: &CompletionItem) -> Self {
        CompletionKey {
            label: item.label.clone(),
            insert_text: item.insert_text.clone(),
        }
    }
}

#[derive(Default)]
struct CompletionWithSource {
    item: CompletionItem,
    sources: Vec<String>,
}

impl CompositeCompletionsSource {
    fn gather_completions(
        &self,
        builder: &CompletionBuilder,
        completions: &mut HashMap<CompletionKey, CompletionWithSource>,
    ) -> Result<()> {
        log::info!("Getting completions from composite sources");

        let always_sources: &[&dyn CompletionSource] = &[
            &call::CallSource,     // argument completions
            &pipe::PipeSource,     // pipe completions, e.g. column names
            &subset::SubsetSource, // subset completions (`[` or `[[`)
        ];
        for source in always_sources {
            let source_name = source.name();
            log::debug!("Trying completions from source: {}", source_name);

            if let Some(source_completions) = source.provide_completions(builder)? {
                log::debug!(
                    "Found {} completions from source: {}",
                    source_completions.len(),
                    source_name
                );
                self.add_completions(source_name, source_completions, completions);
            }
        }

        // Call, pipe, and subset completions should show up no matter what when
        // the user requests completions (this allows them to Tab their way through
        // completions effectively without typing anything). For the rest of the
        // general completions, we require an identifier to begin showing
        // anything.
        if is_identifier_like(builder.context.node) {
            let identifier_only_sources: &[&dyn CompletionSource] = &[
                &keyword::KeywordSource,
                &snippets::SnippetSource,
                &search_path::SearchPathSource,
                &document::DocumentSource,
                &workspace::WorkspaceSource,
            ];

            for source in identifier_only_sources {
                let source_name = source.name();
                log::debug!("Trying completions from source: {}", source_name);

                if let Some(source_completions) = source.provide_completions(builder)? {
                    log::debug!(
                        "Found {} completions from source: {}",
                        source_completions.len(),
                        source_name
                    );
                    self.add_completions(source_name, source_completions, completions);
                }
            }
        }

        Ok(())
    }

    fn add_completions(
        &self,
        source_name: &str,
        items: Vec<CompletionItem>,
        completions: &mut HashMap<CompletionKey, CompletionWithSource>,
    ) {
        for item in items {
            let key = CompletionKey::from(&item);

            if let Some(sourced_item) = completions.get_mut(&key) {
                // Item already exists, just add this source
                sourced_item.sources.push(source_name.to_string());
                log::debug!(
                    "Completion '{}' contributed by multiple sources: {:?}",
                    key.label,
                    sourced_item.sources
                );
            } else {
                // New item
                let mut sourced_item = CompletionWithSource::default();
                sourced_item.item = item;
                sourced_item.sources.push(source_name.to_string());
                completions.insert(key, sourced_item);
            }
        }
    }

    // Produce and sort plain ol' CompletionItems, with source information
    // stashed in `data`. I'm not entirely sure if we really want to even hold
    // on to this information, but it's here for now.
    fn get_completions(
        &self,
        completions: &HashMap<CompletionKey, CompletionWithSource>,
    ) -> Vec<CompletionItem> {
        let mut items: Vec<CompletionItem> = completions
            .values()
            .map(|sourced_item| {
                let mut item = sourced_item.item.clone();

                // Store source information in the `data` field as a simple string
                // if !sourced_item.sources.is_empty() {
                //     let sources_string = sourced_item.sources.join(",");
                //     item.data = Some(serde_json::Value::String(sources_string.clone()));
                //     log::debug!(
                //         "Completion '{}' has sources: {}",
                //         item.label,
                //         sources_string
                //     );
                // }

                item
            })
            .collect();

        // Sort the completions
        Self::sort_completions(&mut items);

        items
    }

    // Sort completions by providing custom 'sort' text to be used when
    // ordering completion results. we use some placeholders at the front
    // to 'bin' different completion types differently; e.g. we place parameter
    // completions at the front, followed by variable completions (like pipe
    // completions and subset completions), followed by anything else.
    fn sort_completions(completions: &mut Vec<CompletionItem>) {
        use tower_lsp::lsp_types::CompletionItemKind;

        for item in completions {
            // Start with existing `sort_text` if one exists
            let sort_text = item.sort_text.take();
            let sort_text = match sort_text {
                Some(sort_text) => sort_text,
                None => item.label.clone(),
            };

            case! {
                // Argument name
                item.kind == Some(CompletionItemKind::FIELD) => {
                    item.sort_text = Some(join!["1-", sort_text]);
                }
                // Something like pipe completions, or data frame column names
                item.kind == Some(CompletionItemKind::VARIABLE) => {
                    item.sort_text = Some(join!["2-", sort_text]);
                }
                // Package names generally have higher preference than function
                // names. Particularly useful for `dev|` to get to `devtools::`,
                // as that has a lot of base R functions with similar names.
                item.kind == Some(CompletionItemKind::MODULE) => {
                    item.sort_text = Some(join!["3-", sort_text]);
                }
                => {
                    item.sort_text = Some(join!["4-", sort_text]);
                }
            }
        }
    }
}

fn is_identifier_like(x: Node) -> bool {
    if x.is_identifier() {
        // Obvious case
        return true;
    }

    // If the user exactly types these keywords, then they end up matching
    // anonymous nodes in the tree-sitter grammar, so they show up as
    // non-`identifier` kinds. However, we do still want to provide completions
    // here, especially in two cases:
    // - `for<tab>` should provide completions for things like `forcats`
    // - `for<tab>` should provide snippet completions for the `for` snippet
    // The keywords here come from matching snippets in `r.code-snippets`.
    if matches!(x.node_type(), NodeType::Anonymous(kind) if matches!(kind.as_str(), "if" | "for" | "while"))
    {
        return true;
    }

    return false;
}

#[cfg(test)]
mod tests {
    use tree_sitter::Point;

    use crate::lsp::completions::sources::composite::is_identifier_like;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::r_task;
    use crate::treesitter::NodeType;
    use crate::treesitter::NodeTypeExt;

    #[test]
    fn test_completions_on_anonymous_node_keywords() {
        r_task(|| {
            // `if`, `for`, and `while` in particular are both tree-sitter
            // anonymous nodes and snippet keywords, so they need to look like
            // identifiers that we provide completions for
            for keyword in ["if", "for", "while"] {
                let point = Point { row: 0, column: 0 };
                let document = Document::new(keyword, None);
                let context = DocumentContext::new(&document, point, None);
                assert!(is_identifier_like(context.node));
                assert_eq!(
                    context.node.node_type(),
                    NodeType::Anonymous(keyword.to_string())
                );
            }
        })
    }
}
