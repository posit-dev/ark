//
// subset.rs
//
// Copyright (C) 2024-2026 Posit Software, PBC. All rights reserved.
//
//

use legacy_ide::traits::node::NodeExt;
use legacy_ide::treesitter::node_find_parent_call;
use legacy_ide::treesitter::NodeTypeExt;
use tree_sitter::Node;

use crate::lsp::completions::sources::common::subset::is_within_subset_delimiters;
use crate::lsp::document_context::DocumentContext;

/// Returns the object identifier for string subsetting, such as `x` in
/// `x["|"]` or `x[c("col", "|")]`, with `|` marking the cursor.
///
/// Strings inside `c()` are treated as object names. Other calls, such as
/// `x[match(foo, "|")]`, may use strings for another purpose and are excluded.
/// Recognition suppresses file path completions even if the object has no names.
///
/// Unlike `composite::subset::recognize_subset()`, this recognizes names
/// inside a string. Those names need no added quotes and are not combined with
/// generic completions.
pub(super) fn node_find_object_for_string_subset<'tree>(
    node: &Node<'tree>,
    context: &DocumentContext,
) -> Option<Node<'tree>> {
    if !node.is_string() {
        return None;
    }

    let mut node = node_find_parent_call(node)?;

    if node.is_call() {
        if !node_is_c_call(&node, context.contents) {
            // Inside a call that isn't `c()`
            return None;
        }

        node = node_find_parent_call(&node)?;

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
    node = node.child_by_field_name("function")?;

    if !node.is_identifier() {
        return None;
    }

    Some(node)
}

fn node_is_c_call(x: &Node, contents: &str) -> bool {
    if !x.is_call() {
        return false;
    }

    let Some(x) = x.child_by_field_name("function") else {
        return false;
    };

    if !x.is_identifier() {
        return false;
    }

    let Ok(text) = x.node_as_str(contents) else {
        log::error!("Can't slice `contents`.");
        return false;
    };

    // Is the call `c()`?
    text == "c"
}

#[cfg(test)]
mod tests {
    use legacy_ide::fixtures::point_from_cursor;
    use legacy_ide::traits::node::NodeExt;
    use legacy_ide::treesitter::node_find_string;

    use crate::lsp::completions::sources::unique::subset::node_find_object_for_string_subset;
    use crate::lsp::document_context::TestDocument;

    fn subset_object_at_cursor(code: &str) -> Option<String> {
        let (text, point) = point_from_cursor(code);
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        let node = node_find_string(&context.node).unwrap();

        node_find_object_for_string_subset(&node, &context)
            .map(|object| object.node_as_str(&text).unwrap().to_string())
    }

    #[test]
    fn test_string_subset_object() {
        // Inside top level `""`
        assert_eq!(
            subset_object_at_cursor(r#"foo["@"]"#),
            Some(String::from("foo"))
        );

        // Inside `""` in `[[`
        assert_eq!(
            subset_object_at_cursor(r#"foo[["@"]]"#),
            Some(String::from("foo"))
        );

        // Inside `""` as second argument
        assert_eq!(
            subset_object_at_cursor(r#"foo[, "@"]"#),
            Some(String::from("foo"))
        );

        // Inside `""` inside `c()`
        assert_eq!(
            subset_object_at_cursor(r#"foo[c("@")]"#),
            Some(String::from("foo"))
        );

        // Inside `""` inside `c()` with another string already specified
        assert_eq!(
            subset_object_at_cursor(r#"foo[c("a", "@")]"#),
            Some(String::from("foo"))
        );

        // Inside `""` inside `fn()` - no completions from string subset!
        // Instead file path completions should kick in, because this is an arbitrary
        // function call so subset completions don't make sense, but file ones might.
        assert_eq!(subset_object_at_cursor(r#"foo[fn("@")]"#), None);

        // Recognition does not require the object to exist in the session.
        assert_eq!(
            subset_object_at_cursor(r#"not_real["@"]"#),
            Some(String::from("not_real"))
        );
    }
}
