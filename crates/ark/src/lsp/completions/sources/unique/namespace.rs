//
// namespace.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::request::UniqueRequest;
use crate::lsp::completions::sources::unique::Recognized;
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

    let exports_only =
        node.node_type() == NodeType::NamespaceOperator(NamespaceOperatorType::External);

    let Some(package) = node.child_by_field_name("lhs") else {
        return Ok(Some(Recognized::Empty));
    };

    let package = package.node_as_str(context.contents)?.to_string();
    let function_context = completion_context.function_context()?.clone();

    Ok(Some(Recognized::Session(UniqueRequest::Namespace {
        package,
        exports_only,
        function_context,
    })))
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
    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::completion_context::CompletionContext;
    use crate::lsp::completions::request::UniqueRequest;
    use crate::lsp::completions::sources::unique::namespace::recognize_namespace;
    use crate::lsp::completions::sources::unique::Recognized;
    use crate::lsp::document_context::TestDocument;
    use crate::lsp::state::WorldState;

    fn recognize_namespace_at_cursor(cursor_text: &str) -> Option<Recognized> {
        let (text, point) = point_from_cursor(cursor_text);
        let doc = TestDocument::new(&text);
        let document_context = doc.context(point);
        let state = WorldState::default().snapshot();
        let context = CompletionContext::new(&document_context, &state);

        recognize_namespace(&context).unwrap()
    }

    fn assert_namespace_slot(cursor_text: &str, expected_package: &str, expected_exports: bool) {
        let Some(Recognized::Session(UniqueRequest::Namespace {
            package,
            exports_only,
            ..
        })) = recognize_namespace_at_cursor(cursor_text)
        else {
            panic!("Expected a namespace slot at `{cursor_text}`");
        };
        assert_eq!(package, expected_package);
        assert_eq!(exports_only, expected_exports);
    }

    #[test]
    fn test_namespace_slot_after_colons() {
        assert_namespace_slot("utils::@", "utils", true);
        assert_namespace_slot("utils:::@", "utils", false);

        // With RHS text, which is ignored when generating completions.
        // Filtering applied on frontend side.
        assert_namespace_slot("utils::bl@ah", "utils", true);

        // Recognition does not require an installed package. An unknown package
        // suppresses later sources (https://github.com/posit-dev/ark/issues/833).
        assert_namespace_slot("foo::bar@", "foo", true);
    }

    #[test]
    fn test_expression_after_colon_colon_doesnt_result_in_completions() {
        assert_eq!(recognize_namespace_at_cursor("base::+@"), None);
    }

    #[test]
    fn test_empty_set_of_completions_when_on_package_name() {
        assert_eq!(
            recognize_namespace_at_cursor("ba@se::ab"),
            Some(Recognized::Empty)
        );
    }

    #[test]
    fn test_empty_set_of_completions_when_not_at_end_of_colons() {
        assert_eq!(
            recognize_namespace_at_cursor("base:@:ab"),
            Some(Recognized::Empty)
        );
        assert_eq!(
            recognize_namespace_at_cursor("base:@::ab"),
            Some(Recognized::Empty)
        );
        assert_eq!(
            recognize_namespace_at_cursor("base::@:ab"),
            Some(Recognized::Empty)
        );
    }
}
