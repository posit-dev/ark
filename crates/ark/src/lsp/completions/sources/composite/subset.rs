//
// subset.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::sources::common::subset::is_within_subset_delimiters;
use crate::lsp::completions::sources::utils::completions_from_evaluated_object_names;
use crate::lsp::document_context::DocumentContext;
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
    let mut needs_completions = false;

    loop {
        let node_type = node.node_type();

        if matches!(node_type, NodeType::Subset | NodeType::Subset2) {
            needs_completions = true;
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

    if !needs_completions {
        // Didn't detect anything worth completing in this context,
        // let other sources add their own candidates instead
        return Ok(None);
    };

    // Only provide subset completions if you are actually within `x[<here>]` or `x[[<here>]]`
    if !is_within_subset_delimiters(&context.point, &node) {
        return Ok(None);
    }

    let Some(child) = node.child(0) else {
        // There is almost definitely a child here. If there isn't,
        // we "tried" to do subset completions but found nothing.
        return Ok(Some(vec![]));
    };

    let text = context.document.contents.node_slice(&child)?.to_string();

    completions_from_evaluated_object_names(&text, ENQUOTE, node.node_type())
}

#[cfg(test)]
mod tests {
    use harp::eval::RParseEvalOptions;
    use tree_sitter::Point;

    use crate::fixtures::package_is_installed;
    use crate::lsp::completions::sources::composite::subset::completions_from_subset;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::r_task;

    #[test]
    fn test_subset_completions() {
        r_task(|| {
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
            assert_eq!(completion.insert_text, Some("\"b\"".to_string()));

            let completion = completions.get(1).unwrap();
            assert_eq!(completion.label, "a".to_string());
            assert_eq!(completion.insert_text, Some("\"a\"".to_string()));

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

    #[test]
    fn test_data_table_subset_completions() {
        r_task(|| {
            if !package_is_installed("data.table") {
                return;
            }

            harp::parse_eval_global("x <- data.table::as.data.table(mtcars)").unwrap();

            // Works for single comlumn name completion
            let point = Point { row: 0, column: 2 };
            let document = Document::new("x[]", None);
            let context = DocumentContext::new(&document, point, None);

            let completions = completions_from_subset(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 11);

            let completion = completions.get(0).unwrap();
            assert_eq!(completion.label, "mpg".to_string());
            assert_eq!(completion.insert_text, None); // No enquote, means the label is used directly

            // Works when completing inside a `c` call
            let point = Point { row: 0, column: 6 };
            let document = Document::new("x[, c()]", None);
            let context = DocumentContext::new(&document, point, None);

            let completions = completions_from_subset(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 11);

            let completion = completions.get(0).unwrap();
            assert_eq!(completion.label, "mpg".to_string());
            assert_eq!(completion.insert_text, None); // No enquote, means the label is used directly

            // Works when completing inside a `c` call with a collumn already selected
            let point = Point { row: 0, column: 10 };
            let document = Document::new("x[, c(mpg,)]", None);
            let context = DocumentContext::new(&document, point, None);

            let completions = completions_from_subset(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 11);

            let completion = completions.get(0).unwrap();
            // TODO: ideally we could assert that mpg doesn't appear again, or appears at the end
            assert_eq!(completion.label, "mpg".to_string());
            assert_eq!(completion.insert_text, None); // No enquote, means the label is used directly

            // Works completing subset2
            let point = Point { row: 0, column: 3 };
            let document = Document::new("x[[]]", None);
            let context = DocumentContext::new(&document, point, None);

            let completions = completions_from_subset(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 11);

            let completion = completions.get(0).unwrap();
            assert_eq!(completion.label, "mpg".to_string());
            assert_eq!(completion.insert_text, Some("\"mpg\"".to_string())); // Enquoted result

            harp::parse_eval_global("remove(x)").unwrap();
        })
    }
}
