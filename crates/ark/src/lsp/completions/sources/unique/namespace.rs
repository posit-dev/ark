//
// namespace.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_symbol;
use libr::R_UnboundValue;
use libr::R_lsInternal;
use libr::Rboolean_TRUE;
use libr::Rf_findVarInFrame;
use libr::SEXP;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::completions::completion_context::CompletionContext;
use crate::lsp::completions::completion_item::completion_item_from_lazydata;
use crate::lsp::completions::completion_item::completion_item_from_namespace;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;
use crate::lsp::completions::sources::CompletionSource;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::NamespaceOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

pub(super) struct NamespaceSource;

impl CompletionSource for NamespaceSource {
    fn name(&self) -> &'static str {
        "namespace"
    }

    fn provide_completions(
        &self,
        completion_context: &CompletionContext,
    ) -> anyhow::Result<Option<Vec<CompletionItem>>> {
        completions_from_namespace(completion_context)
    }
}

// Handle the case with 'package::prefix', where the user has now
// started typing the prefix of the symbol they would like completions for.
fn completions_from_namespace(
    completion_context: &CompletionContext,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
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

    let mut completions: Vec<CompletionItem> = vec![];

    let node = match node {
        NamespaceNodeKind::None => return Ok(None),
        NamespaceNodeKind::EmptySet => return Ok(Some(completions)),
        NamespaceNodeKind::Node(node) => node,
    };

    let exports_only =
        node.node_type() == NodeType::NamespaceOperator(NamespaceOperatorType::External);

    let Some(package) = node.child_by_field_name("lhs") else {
        return Ok(Some(completions));
    };

    let package = context.document.contents.node_slice(&package)?.to_string();
    let package = package.as_str();

    // Get the package namespace
    let Ok(namespace) = RFunction::new("base", "getNamespace").add(package).call() else {
        // There is no package of this name or it could not be loaded, but it did look
        // like the user wanted namespace completions, so disallow anything else from
        // running
        return Ok(Some(completions));
    };

    let symbols = if package == "base" {
        list_namespace_symbols(*namespace)
    } else if exports_only {
        list_namespace_exports(*namespace)
    } else {
        list_namespace_symbols(*namespace)
    };

    let strings = unsafe { symbols.to::<Vec<String>>()? };

    for string in strings.iter() {
        let item = unsafe {
            completion_item_from_namespace(
                string,
                *namespace,
                package,
                completion_context.function_context(),
            )
        };
        match item {
            Ok(item) => completions.push(item),
            Err(error) => log::error!("{error:?}"),
        }
    }

    if exports_only {
        // `pkg:::object` doesn't return lazy objects, so we don't want
        // to show lazydata completions if we are inside `:::`
        let lazydata = completions_from_namespace_lazydata(*namespace, package)?;
        if let Some(mut lazydata) = lazydata {
            completions.append(&mut lazydata);
        }
    }

    set_sort_text_by_words_first(&mut completions);

    Ok(Some(completions))
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

fn completions_from_namespace_lazydata(
    namespace: SEXP,
    package: &str,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    unsafe {
        let ns = Rf_findVarInFrame(namespace, r_symbol!(".__NAMESPACE__."));
        if ns == R_UnboundValue {
            return Ok(None);
        }

        let env = Rf_findVarInFrame(ns, r_symbol!("lazydata"));
        if env == R_UnboundValue {
            return Ok(None);
        }

        let names = RObject::to::<Vec<String>>(RObject::from(R_lsInternal(env, Rboolean_TRUE)))?;

        if names.len() == 0 {
            return Ok(None);
        }

        let mut completions: Vec<CompletionItem> = vec![];

        for name in names.iter() {
            match completion_item_from_lazydata(name, env, package) {
                Ok(item) => completions.push(item),
                Err(error) => log::error!("{:?}", error),
            }
        }

        Ok(Some(completions))
    }
}

fn list_namespace_symbols(namespace: SEXP) -> RObject {
    return unsafe { RObject::new(R_lsInternal(namespace, 1)) };
}

fn list_namespace_exports(namespace: SEXP) -> RObject {
    unsafe {
        let ns = Rf_findVarInFrame(namespace, r_symbol!(".__NAMESPACE__."));
        if ns == R_UnboundValue {
            return RObject::null();
        }

        let exports = Rf_findVarInFrame(ns, r_symbol!("exports"));
        if exports == R_UnboundValue {
            return RObject::null();
        }

        return RObject::new(R_lsInternal(exports, 1));
    }
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::CompletionItem;

    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::completion_context::CompletionContext;
    use crate::lsp::completions::sources::unique::namespace::completions_from_namespace;
    use crate::lsp::completions::tests::utils::find_completion_by_label;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::lsp::state::WorldState;
    use crate::r_task;

    pub(crate) fn get_namespace_completions_at_cursor(
        cursor_text: &str,
    ) -> anyhow::Result<Option<Vec<CompletionItem>>> {
        let (text, point) = point_from_cursor(cursor_text);
        let document = Document::new(&text, None);
        let document_context = DocumentContext::new(&document, point, None);
        let state = WorldState::default();
        let context = CompletionContext::new(&document_context, &state);

        completions_from_namespace(&context)
    }

    #[test]
    fn test_completions_after_colons() {
        r_task(|| {
            // Just colons, no RHS text yet
            let completions = get_namespace_completions_at_cursor("utils::@")
                .unwrap()
                .unwrap();

            let item = find_completion_by_label(&completions, "adist");
            assert!(item.is_some());

            // Should not find internal function
            let item = find_completion_by_label(&completions, "as.bibentry.bibentry");
            assert!(item.is_none());

            // Internal functions with `:::`
            let completions = get_namespace_completions_at_cursor("utils:::@")
                .unwrap()
                .unwrap();
            let item = find_completion_by_label(&completions, "as.bibentry.bibentry");
            assert!(item.is_some());

            // With RHS text, which is ignored when generating completions.
            // Filtering applied on frontend side.
            let completions = get_namespace_completions_at_cursor("utils::bl@ah")
                .unwrap()
                .unwrap();
            let item = find_completion_by_label(&completions, "adist");
            assert!(item.is_some());
        });
    }

    #[test]
    fn test_expression_after_colon_colon_doesnt_result_in_completions() {
        r_task(|| {
            let completions = get_namespace_completions_at_cursor("base::+@").unwrap();
            assert!(completions.is_none());
        });
    }

    #[test]
    fn test_empty_set_of_completions_when_on_package_name() {
        r_task(|| {
            let completions = get_namespace_completions_at_cursor("ba@se::ab")
                .unwrap()
                .unwrap();
            assert!(completions.is_empty());
        });
    }

    #[test]
    fn test_empty_set_of_completions_when_not_at_end_of_colons() {
        r_task(|| {
            let completions = get_namespace_completions_at_cursor("base:@:ab")
                .unwrap()
                .unwrap();
            assert!(completions.is_empty());

            let completions = get_namespace_completions_at_cursor("base:@::ab")
                .unwrap()
                .unwrap();
            assert!(completions.is_empty());

            let completions = get_namespace_completions_at_cursor("base::@:ab")
                .unwrap()
                .unwrap();
            assert!(completions.is_empty());
        });
    }

    #[test]
    fn test_empty_set_of_completions_when_using_unknown_package() {
        // https://github.com/posit-dev/ark/issues/833
        r_task(|| {
            let completions = get_namespace_completions_at_cursor("foo::bar@")
                .unwrap()
                .unwrap();
            assert!(completions.is_empty());
        });
    }
}
