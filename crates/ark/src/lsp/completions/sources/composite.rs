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

use indexmap::IndexMap;
use stdext::*;
use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::CompletionItemKind;
use tree_sitter::Node;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::sources::collect_completions;
use crate::lsp::completions::sources::utils::has_priority_prefix;
use crate::lsp::completions::sources::CompletionSource;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

const OBJECT_KIND_CLASS: &str = "Object";

#[derive(Hash, PartialEq, Eq)]
struct CompletionItemKey {
    label: String,
    kind_class: String,
    // Object kinds only, so that a quoted subset column and an unquoted
    // object with the same name are both kept
    insert_text: Option<String>,
}

impl CompletionItemKey {
    fn new(item: &CompletionItem) -> Self {
        let kind_class = dedupe_kind(item.kind);

        let insert_text = (kind_class == OBJECT_KIND_CLASS).then(|| {
            item.insert_text
                .clone()
                .unwrap_or_else(|| item.label.clone())
        });

        Self {
            label: item.label.clone(),
            kind_class,
            insert_text,
        }
    }
}

// The same R object comes back as `VARIABLE` (document), `STRUCT` (search
// path), or `VALUE` (workspace)
fn dedupe_kind(kind: Option<CompletionItemKind>) -> String {
    match kind {
        Some(CompletionItemKind::VARIABLE) |
        Some(CompletionItemKind::STRUCT) |
        Some(CompletionItemKind::VALUE) => OBJECT_KIND_CLASS.to_string(),
        Some(kind) => format!("{kind:?}"),
        None => "Text".to_string(),
    }
}

// Locally useful data structure for tracking completions and their source
struct CompletionItemWithSource {
    item: CompletionItem,
    source: String,
}

/// Gets completions from all composite sources, with deduplication and sorting
///
/// The first source pushed wins a duplicate, so push order matters. A later
/// `VARIABLE` duplicate still gives the kept item its kind and `sort_text`.
pub(crate) fn get_completions(
    completion_context: &CompletionContext,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    log::info!("Getting completions from composite sources");

    let mut completions = IndexMap::new();

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
    completions: &mut IndexMap<CompletionItemKey, CompletionItemWithSource>,
) -> anyhow::Result<()>
where
    S: CompletionSource,
{
    let source_name = source.name();

    if let Some(source_completions) = collect_completions(source, completion_context)? {
        insert_completions(source_name, source_completions, completions);
    }

    Ok(())
}

fn insert_completions(
    source_name: &str,
    items: Vec<CompletionItem>,
    completions: &mut IndexMap<CompletionItemKey, CompletionItemWithSource>,
) {
    for item in items {
        let key = CompletionItemKey::new(&item);

        let Some(existing) = completions.get_mut(&key) else {
            completions.insert(key, CompletionItemWithSource {
                item,
                source: source_name.to_string(),
            });
            continue;
        };

        log::trace!(
            "Completion with label '{label}' and kind class '{kind_class}' already exists (first contributed by source: {existing_source} as {existing_kind:?}, now also from: {source_name} as {kind:?})",
            label = key.label,
            kind_class = key.kind_class,
            existing_source = existing.source,
            existing_kind = existing.item.kind,
            kind = item.kind,
        );

        // Objects defined in this document keep the `VARIABLE` icon and rank.
        // Take its `sort_text` too, since the search path pre-sets one.
        if item.kind == Some(CompletionItemKind::VARIABLE) &&
            existing.item.kind != Some(CompletionItemKind::VARIABLE)
        {
            existing.item.kind = item.kind;
            existing.item.sort_text = item.sort_text;
        }
    }
}

/// Produce plain old CompletionItems and sort them
fn finalize_completions(
    completions: IndexMap<CompletionItemKey, CompletionItemWithSource>,
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
            Some(sort_text) => {
                if has_priority_prefix(&sort_text) {
                    item.sort_text = Some(sort_text);
                    continue;
                }
                sort_text
            },
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

    false
}

#[cfg(test)]
mod tests {
    use harp::eval::parse_eval_global;
    use indexmap::IndexMap;
    use tower_lsp_server::ls_types::CompletionItem;
    use tower_lsp_server::ls_types::CompletionItemKind;
    use tower_lsp_server::ls_types::CompletionItemLabelDetails;

    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::completion_context::CompletionContext;
    use crate::lsp::completions::sources::composite::finalize_completions;
    use crate::lsp::completions::sources::composite::get_completions;
    use crate::lsp::completions::sources::composite::insert_completions;
    use crate::lsp::completions::sources::composite::is_identifier_like;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::lsp::document_context::TestDocument;
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
                let doc = TestDocument::new(&text);
                let context = doc.context(point);

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
            let doc = TestDocument::new(&text);
            let document_context = doc.context(point);
            let state = WorldState::default().snapshot();
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
            let doc = TestDocument::new(&text);
            let document_context = doc.context(point);
            let state = WorldState::default().snapshot();
            let context = CompletionContext::new(&document_context, &state);

            assert!(context.document_context.node.is_program());

            let completions = get_completions(&context).unwrap();
            assert!(completions.is_some());
            assert!(!completions.unwrap().is_empty());
        });
    }

    #[test]
    fn test_object_kinds_from_different_sources_are_deduped() {
        let mut completions = IndexMap::new();
        insert_completions(
            "search_path",
            vec![item("outPath", CompletionItemKind::STRUCT)],
            &mut completions,
        );
        insert_completions(
            "document",
            vec![item("outPath", CompletionItemKind::VARIABLE)],
            &mut completions,
        );
        insert_completions(
            "workspace",
            vec![item("outPath", CompletionItemKind::VALUE)],
            &mut completions,
        );

        let items = finalize_completions(completions);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
    }

    #[test]
    fn test_object_collision_with_variable_takes_variable_kind_and_sort_bin() {
        let mut search_path_item = item("outPath", CompletionItemKind::STRUCT);
        search_path_item.detail = Some("search path".to_string());
        // The search path source pre-sets `sort_text` to sort words first
        search_path_item.sort_text = Some("1-outPath".to_string());

        let mut document_item = item("outPath", CompletionItemKind::VARIABLE);
        document_item.detail = Some("document".to_string());

        let mut completions = IndexMap::new();
        insert_completions("search_path", vec![search_path_item], &mut completions);
        insert_completions("document", vec![document_item], &mut completions);

        let items = finalize_completions(completions);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].detail, Some("search path".to_string()));
        assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
        assert_eq!(items[0].sort_text, Some("2-outPath".to_string()));
    }

    #[test]
    fn test_variable_collision_with_variable_keeps_own_sort_text() {
        let mut pipe_item = item("xcol", CompletionItemKind::VARIABLE);
        pipe_item.sort_text = Some("xcol-pipe".to_string());

        let mut completions = IndexMap::new();
        insert_completions("pipe", vec![pipe_item], &mut completions);
        insert_completions(
            "document",
            vec![item("xcol", CompletionItemKind::VARIABLE)],
            &mut completions,
        );

        let items = finalize_completions(completions);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].sort_text, Some("2-xcol-pipe".to_string()));
    }

    #[test]
    fn test_objects_with_different_insert_text_are_kept() {
        let mut subset_item = item("x", CompletionItemKind::VARIABLE);
        subset_item.insert_text = Some("\"x\"".to_string());

        let mut completions = IndexMap::new();
        insert_completions("subset", vec![subset_item], &mut completions);
        insert_completions(
            "search_path",
            vec![item("x", CompletionItemKind::STRUCT)],
            &mut completions,
        );

        let items = finalize_completions(completions);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].insert_text, Some("\"x\"".to_string()));
        assert_eq!(items[1].kind, Some(CompletionItemKind::STRUCT));
    }

    #[test]
    fn test_object_without_variable_collision_keeps_own_sort_bin() {
        let mut completions = IndexMap::new();
        insert_completions(
            "search_path",
            vec![item("outPath", CompletionItemKind::STRUCT)],
            &mut completions,
        );
        insert_completions(
            "workspace",
            vec![item("outPath", CompletionItemKind::VALUE)],
            &mut completions,
        );

        let items = finalize_completions(completions);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].kind, Some(CompletionItemKind::STRUCT));
        assert_eq!(items[0].sort_text, Some("4-outPath".to_string()));
    }

    #[test]
    fn test_keyword_and_snippet_with_same_label_are_kept() {
        let mut completions = IndexMap::new();
        insert_completions(
            "keyword",
            vec![
                item("for", CompletionItemKind::KEYWORD),
                item("for", CompletionItemKind::SNIPPET),
            ],
            &mut completions,
        );

        let items = finalize_completions(completions);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, Some(CompletionItemKind::KEYWORD));
        assert_eq!(items[1].kind, Some(CompletionItemKind::SNIPPET));
    }

    #[test]
    fn test_module_and_function_with_same_label_are_kept() {
        let mut completions = IndexMap::new();
        insert_completions(
            "search_path",
            vec![
                item("glue", CompletionItemKind::MODULE),
                item("glue", CompletionItemKind::FUNCTION),
            ],
            &mut completions,
        );

        let items = finalize_completions(completions);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].kind, Some(CompletionItemKind::MODULE));
        assert_eq!(items[1].kind, Some(CompletionItemKind::FUNCTION));
    }

    #[test]
    fn test_object_from_document_and_global_env_is_deduped() {
        r_task(|| {
            parse_eval_global("outPath <- 'myDirectory'").unwrap();

            // Mimics retyping in the editor, where every request must give the
            // same item so the icon and position don't change
            for _ in 0..5 {
                let items = completions_with_label("outPath <- 'myDirectory'\nout@", "outPath");
                assert_eq!(items.len(), 1);
                assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
                assert_eq!(items[0].sort_text, Some("2-outPath".to_string()));
                // Kept the search path item, the only source that sets label
                // details (empty for the global env)
                assert_eq!(
                    items[0].label_details,
                    Some(CompletionItemLabelDetails {
                        detail: None,
                        description: None,
                    })
                );
            }
        });
    }

    #[test]
    fn test_document_only_object_keeps_variable_kind() {
        r_task(|| {
            let items = completions_with_label("outPath <- 'myDirectory'\nout@", "outPath");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
        });
    }

    #[test]
    fn test_pipe_column_wins_over_global_object() {
        r_task(|| {
            parse_eval_global("df <- data.frame(xcol = 1); xcol <- 1").unwrap();

            let items = completions_with_label("df |> foo(xcol@)", "xcol");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
            assert_eq!(items[0].detail, Some("df".to_string()));
        });
    }

    #[test]
    fn test_subset_column_and_global_object_are_both_kept() {
        r_task(|| {
            parse_eval_global("df <- data.frame(x = 1); x <- 1").unwrap();

            // Subset inserts `"x"`, search path inserts `x`
            let items = completions_with_label("df[x@]", "x");
            assert_eq!(items.len(), 2);
            assert_eq!(items[0].insert_text, Some("\"x\"".to_string()));
            assert_eq!(items[0].detail, Some("df".to_string()));
            assert_eq!(items[1].kind, Some(CompletionItemKind::STRUCT));
            assert_eq!(items[1].insert_text, None);
        });
    }

    fn item(label: &str, kind: CompletionItemKind) -> CompletionItem {
        CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            ..Default::default()
        }
    }

    fn completions_with_label(code: &str, label: &str) -> Vec<CompletionItem> {
        get_completions_at_cursor(code)
            .unwrap()
            .into_iter()
            .filter(|item| item.label == label)
            .collect()
    }
}
