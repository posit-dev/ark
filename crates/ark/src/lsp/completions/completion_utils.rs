use std::cmp;

use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;

use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::node_text;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;
use crate::treesitter::UnaryOperatorType;

pub(super) fn log_completions(completions: &Vec<CompletionItem>, source: &str) {
    let count = completions.len();
    let display_count = cmp::min(count, 10);
    log::info!("{} items: {}", source, count);

    let mut insert_text: Vec<String> = completions
        .iter()
        .take(display_count)
        .map(|item| {
            item.insert_text
                .clone()
                .unwrap_or_else(|| item.label.clone())
        })
        .collect();

    if count > display_count {
        insert_text.push(format!("...and {} more", count - display_count));
    }

    if !insert_text.is_empty() {
        log::info!("{} insert_text:\n{}", source, insert_text.join("\n"));
    }
}

#[allow(dead_code)]
pub struct NodeContext<'a> {
    pub node: Node<'a>,
    pub node_text: String,
    pub parent_node: Option<Node<'a>>,
    pub parent_node_text: String,
    pub grandparent_node: Option<Node<'a>>,
    pub grandparent_node_text: String,
    pub greatgrandparent_node: Option<Node<'a>>,
    pub greatgrandparent_node_text: String,
}

pub fn gather_completion_context<'a>(context: &'a DocumentContext<'a>) -> NodeContext<'a> {
    let mut node = context.node;
    // trailing underscore to avoid conflict with the node_text function
    let mut node_text_ = node_text(&node, &context.document.contents).unwrap_or_default();

    let mut parent_node = None;
    let mut parent_node_text = String::new();
    let mut grandparent_node = None;
    let mut grandparent_node_text = String::new();
    let mut greatgrandparent_node = None;
    let mut greatgrandparent_node_text = String::new();

    if let Some(mut parent) = node.parent() {
        // if we are completing "thi" as part of "pkgname::thi", the node we want to
        // start walking up the AST from is the parent node
        if parent.is_namespace_operator() {
            node = parent;
            node_text_ = node_text(&node, &context.document.contents).unwrap_or_default();
            parent = node.parent().unwrap();
        }

        parent_node_text = node_text(&parent, &context.document.contents).unwrap_or_default();
        parent_node = Some(parent);

        if let Some(grandparent) = parent.parent() {
            grandparent_node_text =
                node_text(&grandparent, &context.document.contents).unwrap_or_default();
            grandparent_node = Some(grandparent);

            if let Some(greatgrandparent) = grandparent.parent() {
                greatgrandparent_node_text =
                    node_text(&greatgrandparent, &context.document.contents).unwrap_or_default();
                greatgrandparent_node = Some(greatgrandparent);
            }
        }
    }

    NodeContext {
        node,
        node_text: node_text_,
        parent_node,
        parent_node_text,
        grandparent_node,
        grandparent_node_text,
        greatgrandparent_node,
        greatgrandparent_node_text,
    }
}

pub fn check_for_function_value(context: &DocumentContext, node_context: &NodeContext) -> bool {
    if !matches!(node_context.parent_node, Some(node) if node.is_argument()) {
        return false;
    }

    if !matches!(node_context.grandparent_node, Some(node) if node.is_arguments()) {
        return false;
    }

    let greatgrandparent_node = match node_context.greatgrandparent_node {
        Some(node) if node.is_call() => node,
        _ => return false,
    };

    let function_name_node = match greatgrandparent_node.child_by_field_name("function") {
        Some(node) => node,
        None => return false,
    };

    let function_name = match context
        .document
        .contents
        .node_slice(&function_name_node)
        .ok()
    {
        Some(slice) => slice.to_string(),
        None => return false,
    };

    let target_functions = ["help", "str", "args", "debug", "debugonce", "trace"];
    target_functions.contains(&function_name.as_str())
}

pub fn check_for_help(node_context: &NodeContext) -> bool {
    if node_context.parent_node.is_none() {
        return false;
    }

    let parent_node = node_context.parent_node.unwrap();

    if !parent_node.is_unary_operator() {
        return false;
    }

    if !matches!(
        parent_node.node_type(),
        NodeType::UnaryOperator(UnaryOperatorType::Help)
    ) {
        return false;
    }

    true
}
