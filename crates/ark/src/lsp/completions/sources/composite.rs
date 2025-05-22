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
mod subset;
mod workspace;

use std::collections::HashMap;

use stdext::*;
use tower_lsp::lsp_types::CompletionItem;
use tower_lsp::lsp_types::CompletionItemKind;
use tree_sitter::Node;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::sources::collect_completions;
use crate::lsp::completions::sources::CompletionSource;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

#[derive(Clone, Hash, PartialEq, Eq)]
struct CompletionItemKey {
    label: String,
    kind_str: String,
}

impl CompletionItemKey {
    fn new(item: &CompletionItem) -> Self {
        Self {
            label: item.label.clone(),
            kind_str: item
                .kind
                .map_or_else(|| "Text".to_string(), |k| format!("{:?}", k)),
        }
    }
}

// Locally useful data structure for tracking completions and their source
#[derive(Clone, Default)]
struct CompletionItemWithSource {
    item: CompletionItem,
    source: String,
}

/// Gets completions from all composite sources, with deduplication and sorting
pub(crate) fn get_completions(
    completion_context: &CompletionContext,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    log::info!("Getting completions from composite sources");

    let mut completions = HashMap::new();

    // Call, pipe, and subset completions should show up no matter what when
    // the user requests completions. This allows them to "tab" their way
    // through completions effectively without typing anything.

    // argument completions
    push_completions(call::CallSource, completion_context, &mut completions)?;

    // pipe completions, such as column names of a data frame
    push_completions(pipe::PipeSource, completion_context, &mut completions)?;

    // subset completions (`[` or `[[`)
    push_completions(subset::SubsetSource, completion_context, &mut completions)?;

    // To offer the rest of the general completions, we should be completing:
    // * on an empty line, outside of any function or expression, or
    // * something that looks like an identifier
    if completion_context.document_context.node.is_program() ||
        is_identifier_like(completion_context.document_context.node)
    {
        push_completions(keyword::KeywordSource, completion_context, &mut completions)?;

        push_completions(
            search_path::SearchPathSource,
            completion_context,
            &mut completions,
        )?;

        push_completions(
            document::DocumentSource,
            completion_context,
            &mut completions,
        )?;

        push_completions(
            workspace::WorkspaceSource,
            completion_context,
            &mut completions,
        )?;
    }

    // Simplify to plain old CompletionItems and sort them
    let completions = finalize_completions(completions);

    Ok(Some(completions))
}

fn push_completions<S>(
    source: S,
    completion_context: &CompletionContext,
    completions: &mut HashMap<CompletionItemKey, CompletionItemWithSource>,
) -> anyhow::Result<()>
where
    S: CompletionSource,
{
    let source_name = source.name();

    if let Some(source_completions) = collect_completions(source, completion_context)? {
        for item in source_completions {
            let key = CompletionItemKey::new(&item);
            if let Some(existing) = completions.get(&key) {
                log::trace!(
                    "Completion with label '{}' and kind '{:?}' already exists (first contributed by source: {}, now also from: {})",
                    key.label,
                    key.kind_str,
                    existing.source,
                    source_name
                );
            } else {
                completions.insert(key, CompletionItemWithSource {
                    item,
                    source: source_name.to_string(),
                });
            }
        }
    }

    Ok(())
}

/// Produce plain old CompletionItems and sort them
fn finalize_completions(
    completions: HashMap<CompletionItemKey, CompletionItemWithSource>,
) -> Vec<CompletionItem> {
    let mut items: Vec<CompletionItem> = completions
        .into_values()
        .map(|completion_with_source| completion_with_source.item)
        .collect();

    sort_completions(&mut items);

    items
}

// Sort completions by providing custom 'sort' text to be used when
// ordering completion results. we use some placeholders at the front
// to 'bin' different completion types differently; e.g. we place parameter
// completions at the front, followed by variable completions (like pipe
// completions and subset completions), followed by anything else.
fn sort_completions(completions: &mut Vec<CompletionItem>) {
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
    // - completions of certain reserved words from the keyword source
    if matches!(x.node_type(), NodeType::Anonymous(kind) if matches!(kind.as_str(), "if" | "for" | "while"))
    {
        return true;
    }

    return false;
}

#[cfg(test)]
mod tests {
    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::completion_context::CompletionContext;
    use crate::lsp::completions::sources::composite::get_completions;
    use crate::lsp::completions::sources::composite::is_identifier_like;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::lsp::state::WorldState;
    use crate::r_task;
    use crate::treesitter::NodeType;
    use crate::treesitter::NodeTypeExt;

    #[test]
    fn test_completions_on_anonymous_node_keywords() {
        r_task(|| {
            // `if`, `for`, and `while` in particular are both tree-sitter
            // anonymous nodes and keywords, so they need to look like
            // identifiers that we provide completions for
            for keyword in ["if", "for", "while"] {
                let (text, point) = point_from_cursor(&format!("{keyword}@"));
                let document = Document::new(text.as_str(), None);
                let context = DocumentContext::new(&document, point, None);

                assert!(is_identifier_like(context.node));
                assert_eq!(
                    context.node.node_type(),
                    NodeType::Anonymous(keyword.to_string())
                );
            }
        })
    }

    #[test]
    fn test_get_completions_on_empty_document() {
        r_task(|| {
            let (text, point) = point_from_cursor("@");
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let state = WorldState::default();
            let context = CompletionContext::new(&document_context, &state);

            assert!(context.document_context.node.is_program());

            let completions = get_completions(&context).unwrap();
            assert!(completions.is_some());
            assert!(!completions.unwrap().is_empty());
        });
    }

    #[test]
    fn test_get_completions_on_empty_line_in_non_empty_document() {
        r_task(|| {
            let code = "x <- 1:3\n@\nrnorm(3)";
            let (text, point) = point_from_cursor(code);
            let document = Document::new(text.as_str(), None);
            let document_context = DocumentContext::new(&document, point, None);
            let state = WorldState::default();
            let context = CompletionContext::new(&document_context, &state);

            assert!(context.document_context.node.is_program());

            let completions = get_completions(&context).unwrap();
            assert!(completions.is_some());
            assert!(!completions.unwrap().is_empty());
        });
    }
}
