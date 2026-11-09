//
// namespace.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use oak_db::BindingKind;
use oak_db::NamespaceVisibility;
use oak_ide::namespace_completions;
use oak_ide::NamespaceItem;
use tower_lsp_server::ls_types::CompletionItem;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::completion_item::completion_item_from_function;
use crate::lsp::completions::completion_item::completion_item_from_namespace_object;
use crate::lsp::completions::function_context::FunctionContext;
use crate::lsp::completions::sources::unique::Recognized;
use crate::lsp::completions::sources::unique::UniqueCompletions;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::NamespaceOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

// Handle the case with 'package::prefix', where the user has now
// started typing the prefix of the symbol they would like completions for.
pub(super) fn recognize_namespace(
    completion_context: &CompletionContext,
) -> anyhow::Result<Option<Recognized>> {
    let context = completion_context.document_context;
    let node = context.node;

    // We expect `DocumentContext` to have drilled down into the CST to the anonymous node,
    // we will find the actual `NamespaceOperator` node here
    let node = match node.node_type() {
        NodeType::Anonymous(kind) if matches!(kind.as_str(), "::" | ":::") => {
            namespace_node_from_colons(node, context.point)
        },
        NodeType::Identifier => namespace_node_from_identifier(node),
        _ => return Ok(None),
    };

    let node = match node {
        NamespaceNodeKind::None => return Ok(None),
        NamespaceNodeKind::EmptySet => return Ok(Some(Recognized::Empty)),
        NamespaceNodeKind::Node(node) => node,
    };

    let visibility =
        if node.node_type() == NodeType::NamespaceOperator(NamespaceOperatorType::External) {
            NamespaceVisibility::Exported
        } else {
            NamespaceVisibility::Internal
        };

    let Some(package) = node.child_by_field_name("lhs") else {
        return Ok(Some(Recognized::Empty));
    };
    let package = package.node_as_str(context.contents)?;

    let db = completion_context.state.db();

    // An unknown package suppresses later sources
    // (https://github.com/posit-dev/ark/issues/833)
    let Some(completions) = namespace_completions(db, package, visibility) else {
        return Ok(Some(Recognized::Empty));
    };

    let function_context = completion_context.function_context()?;
    let items = namespace_items(&completions.items, package, function_context)?;

    Ok(Some(Recognized::Static(UniqueCompletions {
        items,
        incomplete_until_package_settles: completions.incomplete_until_package_settles,
    })))
}

fn namespace_items(
    items: &[NamespaceItem],
    package: &str,
    function_context: &FunctionContext,
) -> anyhow::Result<Vec<CompletionItem>> {
    let mut completions = Vec::with_capacity(items.len());

    for item in items {
        let completion = match item.kind {
            BindingKind::Function => {
                completion_item_from_function(&item.name, Some(package), function_context)?
            },
            BindingKind::Unknown => {
                completion_item_from_namespace_object(&item.name, package, function_context)?
            },
        };
        completions.push(completion);
    }

    set_sort_text_by_words_first(&mut completions);

    Ok(completions)
}

enum NamespaceNodeKind<'tree> {
    /// We aren't in a namespace node, allow other completions to run
    None,
    /// It looks like we are in some kind of namespace node, but something is off.
    /// Don't allow any other completions to run here, anything we show is likely to
    /// be wrong.
    EmptySet,
    /// We found the namespace node
    Node(tree_sitter::Node<'tree>),
}

fn namespace_node_from_colons(node: Node, point: Point) -> NamespaceNodeKind {
    if node.end_position() != point {
        // If we aren't at the end of the anonymous `::`/`:::` node, don't return
        // any completions.
        return NamespaceNodeKind::EmptySet;
    }

    let Some(parent) = node.parent() else {
        // Anonymous `::`/`:::` without a parent? Should not be possible.
        return NamespaceNodeKind::EmptySet;
    };

    if !matches!(parent.node_type(), NodeType::NamespaceOperator(_)) {
        // Anonymous `::`/`:::` without a named `::`/`:::` parent? Should not be possible.
        return NamespaceNodeKind::EmptySet;
    }

    NamespaceNodeKind::Node(parent)
}

fn namespace_node_from_identifier(node: Node) -> NamespaceNodeKind {
    let Some(parent) = node.parent() else {
        // Simple identifier without a parent.
        // Totally possible. Want other completions to have a chance to run.
        return NamespaceNodeKind::None;
    };

    if !parent.is_namespace_operator() {
        // Simple identifier with a parent that isn't a namespace node.
        // Totally possible. Want other completions to have a chance to run.
        return NamespaceNodeKind::None;
    }

    if let Some(lhs) = parent.child_by_field_name("lhs") {
        // If we got here from the LHS of the `::`/`:::` node, then we don't
        // want to provide any completions, because we are sitting on the package name
        // and general completions here are not appropriate.
        // TODO: In theory we can do better, and supply package names here. Possibly
        // we should make a separate "unique" source of completions that runs before
        // this one and targets this exact scenario, i.e. `dp<tab>::across()`.
        if lhs.eq(&node) {
            return NamespaceNodeKind::EmptySet;
        }
    }

    NamespaceNodeKind::Node(parent)
}

#[cfg(test)]
mod tests {
    use oak_db::SourceDb;
    use tower_lsp_server::ls_types::CompletionItem;
    use tower_lsp_server::ls_types::CompletionItemKind;

    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::completion_context::CompletionContext;
    use crate::lsp::completions::finish;
    use crate::lsp::completions::request::RCompletionReplies;
    use crate::lsp::completions::resolve::resolve_target;
    use crate::lsp::completions::resolve::ResolveTarget;
    use crate::lsp::completions::sources::unique::namespace::recognize_namespace;
    use crate::lsp::completions::sources::unique::Recognized;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::completions::tests::utils::state_with_r_library;
    use crate::lsp::completions::tests::utils::state_with_workspace_package;
    use crate::lsp::completions::tests::utils::static_completions_with_state;
    use crate::lsp::completions::tests::utils::write_library_package;
    use crate::lsp::document_context::open_test_document;
    use crate::lsp::document_context::test_document_context;
    use crate::lsp::state::WorldState;

    fn recognize_namespace_at_cursor(
        cursor_text: &str,
        state: &mut WorldState,
    ) -> Option<Recognized> {
        let (text, point) = point_from_cursor(cursor_text);
        let file = open_test_document(state, &text);
        let document_context = test_document_context(state, file, point, None);
        let context = CompletionContext::new(&document_context, file, state);

        recognize_namespace(&context).unwrap()
    }

    /// Require a static answer that depends on library package `package`.
    fn static_items(
        cursor_text: &str,
        state: &mut WorldState,
        package: &str,
    ) -> Vec<CompletionItem> {
        let Some(Recognized::Static(completions)) =
            recognize_namespace_at_cursor(cursor_text, state)
        else {
            panic!("Expected a static namespace answer at `{cursor_text}`");
        };
        assert_eq!(
            completions.incomplete_until_package_settles,
            state.db().package_by_name(package)
        );
        assert!(completions.incomplete_until_package_settles.is_some());
        completions.items
    }

    #[test]
    fn test_namespace_completions_after_colons() {
        let mut state = state_with_r_library();

        let items = static_items("utils::@", &mut state, "utils");
        assert!(find_completion_by_label(&items, "adist").is_some());

        // An S3 method that `utils` registers but doesn't export.
        assert!(find_completion_by_label(&items, "as.bibentry.bibentry").is_none());

        // Without fetched sources, `:::` lists the exports.
        let items = static_items("utils:::@", &mut state, "utils");
        assert!(find_completion_by_label(&items, "adist").is_some());

        // With RHS text, which is ignored when generating completions.
        // Filtering applied on frontend side.
        let items = static_items("utils::bl@ah", &mut state, "utils");
        assert!(find_completion_by_label(&items, "adist").is_some());
    }

    #[test]
    fn test_namespace_completions_need_no_session() {
        let mut state = state_with_r_library();

        let (completions, request) = static_completions_with_state("utils::@", &mut state);
        assert!(!request.has_session_slots());
        assert_eq!(
            completions.incomplete_until_package_settles(),
            state.db().package_by_name("utils")
        );

        let items = finish(completions, RCompletionReplies::default());
        assert!(find_completion_by_label(&items, "adist").is_some());
    }

    #[test]
    fn test_namespace_object_resolves_to_topic() {
        let mut state = state_with_r_library();

        // Without sources, every export is an object documented by its topic.
        let items = static_items("utils::@", &mut state, "utils");
        let item = find_completion_by_label(&items, "osVersion").unwrap();
        assert_eq!(item.kind, Some(CompletionItemKind::STRUCT));
        assert_eq!(
            resolve_target(item).unwrap(),
            Some(ResolveTarget::Topic {
                name: String::from("osVersion"),
                package: String::from("utils"),
            })
        );
    }

    #[test]
    fn test_lazy_data_object_resolves_to_topic() {
        let library = tempfile::TempDir::new().unwrap();
        let mut state = write_library_package(library.path(), "pkg", "", None);
        let package = state.db().package_by_name("pkg").unwrap();
        package.set_lazy_data(state.db_mut(), vec![String::from("mtcars")]);

        let items = static_items("pkg::@", &mut state, "pkg");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "mtcars");
        assert_eq!(items[0].kind, Some(CompletionItemKind::STRUCT));
        assert_eq!(
            resolve_target(&items[0]).unwrap(),
            Some(ResolveTarget::Topic {
                name: String::from("mtcars"),
                package: String::from("pkg"),
            })
        );

        assert!(static_items("pkg:::@", &mut state, "pkg").is_empty());
    }

    #[test]
    fn test_unknown_package_ends_search_without_items() {
        // https://github.com/posit-dev/ark/issues/833
        let mut state = state_with_r_library();
        assert_eq!(
            recognize_namespace_at_cursor("foo::bar@", &mut state),
            Some(Recognized::Empty)
        );

        let (completions, _) = static_completions_with_state("foo::bar@", &mut state);
        assert_eq!(completions.incomplete_until_package_settles(), None);
        assert_eq!(finish(completions, RCompletionReplies::default()), vec![]);
    }

    #[test]
    fn test_workspace_package_doesnt_depend_on_sources() {
        let workspace = tempfile::TempDir::new().unwrap();
        let mut state =
            state_with_workspace_package(workspace.path(), "mypkg", "export(f)\n", &[(
                "f.R",
                "f <- function() NULL\n",
            )]);

        let Some(Recognized::Static(completions)) =
            recognize_namespace_at_cursor("mypkg::@", &mut state)
        else {
            panic!("Expected a static namespace answer");
        };
        assert_eq!(completions.incomplete_until_package_settles, None);
        let items = completions.items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "f");
        assert_eq!(items[0].kind, Some(CompletionItemKind::FUNCTION));
    }

    #[test]
    fn test_library_package_items_take_kinds_from_sources() {
        let library = tempfile::TempDir::new().unwrap();
        let mut state = write_library_package(
            library.path(),
            "pkg",
            "export(f, x, `%op%`)\n",
            Some(&[(
                "f.R",
                "f <- function() NULL\nx <- 1\n`%op%` <- function(a, b) NULL\n",
            )]),
        );

        let items = static_items("pkg::@", &mut state, "pkg");
        let labels: Vec<&str> = items.iter().map(|item| item.label.as_str()).collect();
        assert_eq!(labels, vec!["%op%", "f", "x"]);

        assert_eq!(items[0].kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!(items[1].kind, Some(CompletionItemKind::FUNCTION));
        assert_eq!(items[2].kind, Some(CompletionItemKind::STRUCT));

        // Words sort before operators.
        assert_eq!(items[0].sort_text, Some(String::from("2-%op%")));
        assert_eq!(items[1].sort_text, Some(String::from("1-f")));

        assert_eq!(
            resolve_target(&items[1]).unwrap(),
            Some(ResolveTarget::Function {
                name: String::from("f"),
                package: Some(String::from("pkg")),
            })
        );
    }

    #[test]
    fn test_expression_after_colon_colon_doesnt_result_in_completions() {
        let mut state = WorldState::default();
        assert_eq!(recognize_namespace_at_cursor("base::+@", &mut state), None);
    }

    #[test]
    fn test_empty_set_of_completions_when_on_package_name() {
        let mut state = state_with_r_library();
        assert_eq!(
            recognize_namespace_at_cursor("ba@se::ab", &mut state),
            Some(Recognized::Empty)
        );
    }

    #[test]
    fn test_empty_set_of_completions_when_not_at_end_of_colons() {
        let mut state = state_with_r_library();
        assert_eq!(
            recognize_namespace_at_cursor("base:@:ab", &mut state),
            Some(Recognized::Empty)
        );
        assert_eq!(
            recognize_namespace_at_cursor("base:@::ab", &mut state),
            Some(Recognized::Empty)
        );
        assert_eq!(
            recognize_namespace_at_cursor("base::@:ab", &mut state),
            Some(Recognized::Empty)
        );
    }
}
