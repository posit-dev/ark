//
// subset.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;
use tree_sitter::Point;

use crate::lsp::completions::sources::utils::completions_from_evaluated_object_names;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::point::PointExt;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

/// Checks for `[` and `[[` completions
///
/// `$` and `@` are handled elsewhere as they can't be composed with other
/// completions.
pub(super) fn completions_from_subset(
    context: &DocumentContext,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_subset()");

    const ENQUOTE: bool = true;

    let mut node = context.node;
    let mut subset_type = None;

    loop {
        let node_type = node.node_type();

        if matches!(node_type, NodeType::Subset | NodeType::Subset2) {
            subset_type = Some(node_type);
            break;
        }

        // If we reach a brace list, bail.
        if node.is_braced_expression() {
            break;
        }

        // Update the node.
        node = match node.parent() {
            Some(node) => node,
            None => break,
        };
    }

    let Some(subset_type) = subset_type else {
        // Didn't detect anything worth completing in this context,
        // let other sources add their own candidates instead
        return Ok(None);
    };

    // Only provide subset completions if you are actually within `x[<here>]` or `x[[<here>]]`
    if !is_within_subset_delimiters(&context.point, &node, &subset_type) {
        return Ok(None);
    }

    let Some(child) = node.child(0) else {
        // There is almost definitely a child here. If there isn't,
        // we "tried" to do subset completions but found nothing.
        return Ok(Some(vec![]));
    };

    let text = context.document.contents.node_slice(&child)?.to_string();

    completions_from_evaluated_object_names(&text, ENQUOTE)
}

fn is_within_subset_delimiters(x: &Point, subset_node: &Node, subset_type: &NodeType) -> bool {
    let (open, close) = match subset_type {
        NodeType::Subset => ("[", "]"),
        NodeType::Subset2 => ("[[", "]]"),
        _ => std::unreachable!(),
    };

    let Some(arguments) = subset_node.child_by_field_name("arguments") else {
        return false;
    };

    let n_children = arguments.child_count();

    if n_children < 2 {
        return false;
    }

    let Some(open_node) = arguments.child(1 - 1) else {
        return false;
    };
    let Some(close_node) = arguments.child(n_children - 1) else {
        return false;
    };

    // Ensure open and closing nodes are the right type
    if !matches!(
        open_node.node_type(),
        NodeType::Anonymous(kind) if kind == open
    ) {
        return false;
    }
    if !matches!(
        close_node.node_type(),
        NodeType::Anonymous(kind) if kind == close
    ) {
        return false;
    }

    let contains_start = x.is_after_or_equal(open_node.end_position());
    let contains_end = x.is_before_or_equal(close_node.start_position());

    contains_start && contains_end
}

#[cfg(test)]
mod tests {
    use harp::eval::RParseEvalOptions;
    use tree_sitter::Point;

    use crate::lsp::completions::sources::composite::subset::completions_from_subset;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::test::r_test;

    #[test]
    fn test_subset_completions() {
        r_test(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            // Set up a list with names
            harp::parse_eval("foo <- list(b = 1, a = 2)", options.clone()).unwrap();

            // Right after the `[`
            let point = Point { row: 0, column: 4 };
            let document = Document::new("foo[]", None);
            let context = DocumentContext::new(&document, point, None);

            let completions = completions_from_subset(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 2);

            let completion = completions.get(0).unwrap();
            assert_eq!(completion.label, "b".to_string());

            let completion = completions.get(1).unwrap();
            assert_eq!(completion.label, "a".to_string());

            // Right before the `[`
            let point = Point { row: 0, column: 3 };
            let document = Document::new("foo[]", None);
            let context = DocumentContext::new(&document, point, None);
            let completions = completions_from_subset(&context).unwrap();
            assert!(completions.is_none());

            // Right after the `]`
            let point = Point { row: 0, column: 5 };
            let document = Document::new("foo[]", None);
            let context = DocumentContext::new(&document, point, None);
            let completions = completions_from_subset(&context).unwrap();
            assert!(completions.is_none());

            // Clean up
            harp::parse_eval("remove(foo)", options.clone()).unwrap();
        })
    }
}
