//
// composite.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

mod call;
mod document;
mod keyword;
mod pipe;
mod search_path;
mod subset;
mod workspace;

use indexmap::IndexMap;
use stdext::*;
use tower_lsp_server::ls_types::CompletionItem;
use tower_lsp_server::ls_types::CompletionItemKind;
use tree_sitter::Node;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::request::CompositeReplies;
use crate::lsp::completions::request::CompositeRequest;
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

/// Static items of each composite source, merged with the session's replies
/// by [CompositeCompletions::merge()]. The pipe, subset and search path slots
/// have no static items.
#[derive(Debug, Default)]
pub(crate) struct CompositeCompletions {
    /// Formals of a workspace function named like the callee.
    call: Vec<CompletionItem>,
    keyword: Vec<CompletionItem>,
    document: Vec<CompletionItem>,
    workspace: Vec<CompletionItem>,
}

/// Collects static items from composite sources, and the session slots they
/// need filled.
pub(crate) fn get_completions(
    completion_context: &CompletionContext,
) -> anyhow::Result<(CompositeCompletions, CompositeRequest)> {
    log::info!("Getting completions from composite sources");

    let mut completions = CompositeCompletions::default();

    // Call, pipe, and subset completions should show up no matter what when
    // the user requests completions. This allows them to "tab" their way
    // through completions effectively without typing anything.

    // The root supplies both pipe-name completions and the call's S3 dispatch object.
    let pipe_root = pipe::find_pipe_root(completion_context);

    let call = call::recognize_call(completion_context, pipe_root.is_some())?;
    if let Some(call) = &call {
        completions.call = call::completions_from_workspace_arguments(
            completion_context.state.db(),
            &call.callee,
            call.position,
        )?;
    }

    // subset completions (`[` or `[[`)
    let subset = subset::recognize_subset(completion_context.document_context)?;

    // To offer the rest of the general completions, we should be completing:
    // * on an empty line, outside of any function or expression, or
    // * something that looks like an identifier
    let mut search_path = None;
    if completion_context.document_context.node.is_program() ||
        is_identifier_like(completion_context.document_context.node)
    {
        completions.keyword = static_items(keyword::KeywordSource, completion_context)?;
        search_path = Some(search_path::search_path_request(completion_context)?);
        completions.document = static_items(document::DocumentSource, completion_context)?;
        completions.workspace = static_items(workspace::WorkspaceSource, completion_context)?;
    }

    let request = CompositeRequest {
        pipe_root,
        call,
        subset,
        search_path,
    };

    Ok((completions, request))
}

fn static_items<S>(
    source: S,
    completion_context: &CompletionContext,
) -> anyhow::Result<Vec<CompletionItem>>
where
    S: CompletionSource,
{
    Ok(collect_completions(source, completion_context)?.unwrap_or_default())
}

impl CompositeCompletions {
    /// Merges static items and session replies, with deduplication and sorting.
    ///
    /// For items with the same [CompletionItemKey], the first source wins,
    /// except that a later `VARIABLE` replaces a non-`VARIABLE` item. Session
    /// items replace their slot's static items before deduplication, so source
    /// precedence does not depend on whether items are static or session-derived.
    pub(crate) fn merge(self, replies: CompositeReplies) -> Vec<CompletionItem> {
        let CompositeCompletions {
            call,
            keyword,
            document,
            workspace,
        } = self;
        let CompositeReplies {
            call: call_reply,
            pipe: pipe_reply,
            subset: subset_reply,
            search_path: search_path_reply,
        } = replies;

        let mut completions = IndexMap::new();

        insert_completions("call", call_reply.items_or(call), &mut completions);
        insert_completions("pipe", pipe_reply.items_or(vec![]), &mut completions);
        insert_completions("subset", subset_reply.items_or(vec![]), &mut completions);
        insert_completions("keyword", keyword, &mut completions);
        insert_completions(
            "search_path",
            search_path_reply.items_or(vec![]),
            &mut completions,
        );
        insert_completions("document", document, &mut completions);
        insert_completions("workspace", workspace, &mut completions);

        finalize_completions(completions)
    }
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

        // A `VARIABLE` from this document (an assignment or a parameter)
        // replaces the kept item, so it shows its own icon, rank, and docs
        if item.kind == Some(CompletionItemKind::VARIABLE) &&
            existing.item.kind != Some(CompletionItemKind::VARIABLE)
        {
            existing.item = item;
            existing.source = source_name.to_string();
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
    use indexmap::IndexMap;
    use tower_lsp_server::ls_types::CompletionItem;
    use tower_lsp_server::ls_types::CompletionItemKind;

    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::completion_context::CompletionContext;
    use crate::lsp::completions::request::CompositeReplies;
    use crate::lsp::completions::request::SlotReply;
    use crate::lsp::completions::sources::composite::finalize_completions;
    use crate::lsp::completions::sources::composite::get_completions;
    use crate::lsp::completions::sources::composite::insert_completions;
    use crate::lsp::completions::sources::composite::is_identifier_like;
    use crate::lsp::completions::sources::composite::CompositeCompletions;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::document_context::TestDocument;
    use crate::lsp::state::WorldState;
    use crate::treesitter::NodeType;
    use crate::treesitter::NodeTypeExt;

    #[test]
    fn test_completions_on_anonymous_node_keywords() {
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
    }

    #[test]
    fn test_get_completions_on_empty_document() {
        let (text, point) = point_from_cursor("@");
        let doc = TestDocument::new(&text);
        let document_context = doc.context(point);

        assert!(document_context.node.is_program());
        assert!(!static_composite_completions(&document_context).is_empty());
    }

    #[test]
    fn test_get_completions_on_empty_line_in_non_empty_document() {
        let code = "x <- 1:3\n@\nrnorm(3)";
        let (text, point) = point_from_cursor(code);
        let doc = TestDocument::new(&text);
        let document_context = doc.context(point);

        assert!(document_context.node.is_program());
        assert!(!static_composite_completions(&document_context).is_empty());
    }

    #[test]
    fn test_session_reply_keeps_source_precedence() {
        let mut call_item = item("x", CompletionItemKind::VARIABLE);
        call_item.detail = Some("call".to_string());
        let mut pipe_item = item("x", CompletionItemKind::VARIABLE);
        pipe_item.detail = Some("pipe".to_string());

        let completions = CompositeCompletions {
            call: vec![call_item],
            ..Default::default()
        };
        let replies = CompositeReplies {
            call: SlotReply::Decline,
            pipe: SlotReply::Fill(vec![pipe_item]),
            ..Default::default()
        };

        // Call items take precedence over pipe items even when only the pipe
        // slot has a session reply.
        let items = completions.merge(replies);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].detail, Some("call".to_string()));
    }

    #[test]
    fn test_session_fill_replaces_static_slot_items() {
        let completions = CompositeCompletions {
            call: vec![item("workspace", CompletionItemKind::FIELD)],
            ..Default::default()
        };
        let replies = CompositeReplies {
            call: SlotReply::Fill(vec![item("session", CompletionItemKind::FIELD)]),
            ..Default::default()
        };

        let items = completions.merge(replies);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "session");
    }

    #[test]
    fn test_session_search_path_item_is_replaced_by_document_variable() {
        let completions = CompositeCompletions {
            document: vec![item("outPath", CompletionItemKind::VARIABLE)],
            ..Default::default()
        };
        let replies = CompositeReplies {
            search_path: SlotReply::Fill(vec![
                item("outPath", CompletionItemKind::STRUCT),
                item("outer", CompletionItemKind::FUNCTION),
            ]),
            ..Default::default()
        };

        // A document `VARIABLE` replaces a search path duplicate without
        // changing its position in the merged list.
        let items = completions.merge(replies);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].label, "outPath");
        assert_eq!(items[0].kind, Some(CompletionItemKind::VARIABLE));
        assert_eq!(items[1].label, "outer");
    }

    fn static_composite_completions(document_context: &DocumentContext) -> Vec<CompletionItem> {
        let state = WorldState::default();
        let context = CompletionContext::new(document_context, &state);
        let (completions, _) = get_completions(&context).unwrap();

        completions.merge(CompositeReplies::default())
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
    fn test_object_collision_with_variable_is_replaced_by_variable() {
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
        assert_eq!(items[0].detail, Some("document".to_string()));
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

    fn item(label: &str, kind: CompletionItemKind) -> CompletionItem {
        CompletionItem {
            label: label.to_string(),
            kind: Some(kind),
            ..Default::default()
        }
    }
}
