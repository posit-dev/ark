//
// subset.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use ropey::Rope;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;

use crate::lsp::completions::sources::common::subset::is_within_subset_delimiters;
use crate::lsp::completions::sources::utils::completions_from_evaluated_object_names;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::NodeTypeExt;

/// Checks for `[` and `[[` completions when the user is inside a `""`
///
/// This is a _unique_ completions case where we may show the user the object's names if:
/// - We are inside a top level string, like `x["<tab>"]`
/// - We are inside a simple `c()` call, like `x[c("col", "<tab>")]`
///
/// The latter is just a useful heuristic. For more complex function calls, we don't want
/// to populate object names because they won't make sense, like `x[match(foo, "<tab>")]`.
///
/// Different from `composite::subset::completions_from_subset()`, which applies outside
/// of `""`, enquotes its completion items, and is composite so it meshes with other
/// generic completions. We consider this a completely different path.
pub(super) fn completions_from_string_subset(
    node: &Node,
    context: &DocumentContext,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_string_subset()");

    // Already inside a string
    const ENQUOTE: bool = false;

    // i.e. find `x` in `x[""]` or `x[c("foo", "")]`
    let Some(node) = node_find_object_for_string_subset(node, context) else {
        return Ok(None);
    };

    // It looks like we should provide subset completions. Regardless of what happens
    // when getting object names, we should at least return an empty set to stop further
    // completion sources from running.
    let mut completions: Vec<CompletionItem> = vec![];

    let text = context.document.contents.node_slice(&node)?.to_string();

    if let Some(mut candidates) = completions_from_evaluated_object_names(&text, ENQUOTE)? {
        completions.append(&mut candidates);
    }

    Ok(Some(completions))
}

fn node_find_object_for_string_subset<'tree>(
    node: &Node<'tree>,
    context: &DocumentContext,
) -> Option<Node<'tree>> {
    if !node.is_string() {
        return None;
    }

    let mut node = match node_find_parent_call(node) {
        Some(node) => node,
        None => return None,
    };

    if node.is_call() {
        if !node_is_c_call(&node, &context.document.contents) {
            // Inside a call that isn't `c()`
            return None;
        }

        node = match node_find_parent_call(&node) {
            Some(node) => node,
            None => return None,
        };

        if !node.is_subset() && !node.is_subset2() {
            return None;
        }
    }

    // Only provide subset completions if you are actually within `x[<here>]` or `x[[<here>]]`
    if !is_within_subset_delimiters(&context.point, &node) {
        return None;
    }

    // We know `node` is the subset or subset2 node of interest. Return its "function",
    // i.e. likely the object name of interest to extract names for.
    node = match node.child_by_field_name("function") {
        Some(node) => node,
        None => return None,
    };

    if !node.is_identifier() {
        return None;
    }

    return Some(node);
}

fn node_find_parent_call<'tree>(x: &Node<'tree>) -> Option<Node<'tree>> {
    // Find the `Argument` node
    let Some(x) = x.parent() else {
        return None;
    };
    if !x.is_argument() {
        return None;
    }

    // Find the `Arguments` node
    let Some(x) = x.parent() else {
        return None;
    };
    if !x.is_arguments() {
        return None;
    }

    // Find the call node - can be a generic `Call`, `Subset`, or `Subset2`.
    // All 3 purposefully share the same tree structure.
    let Some(x) = x.parent() else {
        return None;
    };
    if !x.is_call() && !x.is_subset() && !x.is_subset2() {
        return None;
    }

    Some(x)
}

fn node_is_c_call(x: &Node, contents: &Rope) -> bool {
    if !x.is_call() {
        return false;
    }

    let Some(x) = x.child_by_field_name("function") else {
        return false;
    };

    if !x.is_identifier() {
        return false;
    }

    let Ok(text) = contents.node_slice(&x) else {
        log::error!("Can't slice `contents`.");
        return false;
    };

    // Is the call `c()`?
    text == "c"
}

#[cfg(test)]
mod tests {
    use harp::eval::parse_eval_global;

    use crate::lsp::completions::sources::unique::subset::completions_from_string_subset;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::test::point_from_cursor;
    use crate::test::r_test;
    use crate::treesitter::node_find_string;

    #[test]
    fn test_string_subset_completions() {
        r_test(|| {
            // Set up a list with names
            parse_eval_global("foo <- list(b = 1, a = 2)").unwrap();

            // Inside top level `""`
            let (text, point) = point_from_cursor(r#"foo["@"]"#);
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let node = node_find_string(&context.node).unwrap();

            let completions = completions_from_string_subset(&node, &context)
                .unwrap()
                .unwrap();
            assert_eq!(completions.len(), 2);

            let completion = completions.get(0).unwrap();
            assert_eq!(completion.label, "b".to_string());
            // Not enquoting, so uses `label` directly
            assert_eq!(completion.insert_text, None);

            let completion = completions.get(1).unwrap();
            assert_eq!(completion.label, "a".to_string());
            // Not enquoting, so uses `label` directly
            assert_eq!(completion.insert_text, None);

            // Inside `""` in `[[`
            let (text, point) = point_from_cursor(r#"foo[["@"]]"#);
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let node = node_find_string(&context.node).unwrap();
            let completions = completions_from_string_subset(&node, &context)
                .unwrap()
                .unwrap();
            assert_eq!(completions.len(), 2);

            // Inside `""` as second argument
            let (text, point) = point_from_cursor(r#"foo[, "@"]"#);
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let node = node_find_string(&context.node).unwrap();
            let completions = completions_from_string_subset(&node, &context)
                .unwrap()
                .unwrap();
            assert_eq!(completions.len(), 2);

            // Inside `""` inside `c()`
            let (text, point) = point_from_cursor(r#"foo[c("@")]"#);
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let node = node_find_string(&context.node).unwrap();
            let completions = completions_from_string_subset(&node, &context)
                .unwrap()
                .unwrap();
            assert_eq!(completions.len(), 2);

            // Inside `""` inside `c()` with another string already specified
            let (text, point) = point_from_cursor(r#"foo[c("a", "@")]"#);
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let node = node_find_string(&context.node).unwrap();
            let completions = completions_from_string_subset(&node, &context)
                .unwrap()
                .unwrap();
            assert_eq!(completions.len(), 2);

            // Inside `""` inside `fn()` - no completions from string subset!
            // Instead file path completions should kick in, because this is an arbitrary
            // function call so subset completions don't make sense, but file ones might.
            let (text, point) = point_from_cursor(r#"foo[fn("@")]"#);
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let node = node_find_string(&context.node).unwrap();
            let completions = completions_from_string_subset(&node, &context).unwrap();
            assert!(completions.is_none());

            // A fake object that we can't get object names for.
            // It _looks_ like we want string completions though, so we return an empty set.
            let (text, point) = point_from_cursor(r#"not_real["@"]"#);
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let node = node_find_string(&context.node).unwrap();
            let completions = completions_from_string_subset(&node, &context)
                .unwrap()
                .unwrap();
            assert!(completions.is_empty());

            // Clean up
            parse_eval_global("remove(foo)").unwrap();
        })
    }

    #[test]
    fn test_string_subset_completions_on_matrix() {
        r_test(|| {
            // Set up a list with names
            parse_eval_global("foo <- array(1, dim = c(2, 2))").unwrap();
            parse_eval_global("colnames(foo) <- c('a', 'b')").unwrap();

            let (text, point) = point_from_cursor(r#"foo[, "@"]"#);
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let node = node_find_string(&context.node).unwrap();

            let completions = completions_from_string_subset(&node, &context)
                .unwrap()
                .unwrap();
            assert_eq!(completions.len(), 2);
            assert_eq!(completions.get(0).unwrap().label, "a".to_string());
            assert_eq!(completions.get(1).unwrap().label, "b".to_string());

            // Clean up
            parse_eval_global("remove(foo)").unwrap();
        })
    }
}
