//
// subset.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use legacy_ide::traits::node::NodeExt;
use legacy_ide::treesitter::NodeType;
use legacy_ide::treesitter::NodeTypeExt;

use crate::lsp::completions::request::SubsetOperator;
use crate::lsp::completions::request::SubsetRequest;
use crate::lsp::completions::sources::common::subset::is_within_subset_delimiters;
use crate::lsp::document_context::DocumentContext;

/// Checks for `[` and `[[` completions
///
/// `$` and `@` are handled elsewhere as they can't be composed with other
/// completions.
pub(super) fn recognize_subset(context: &DocumentContext) -> anyhow::Result<Option<SubsetRequest>> {
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
        return Ok(None);
    };

    let object = child.node_as_str(context.contents)?.to_string();

    let operator = if node.is_subset() {
        SubsetOperator::Single
    } else {
        SubsetOperator::Double
    };

    Ok(Some(SubsetRequest { object, operator }))
}

#[cfg(test)]
mod tests {
    use tree_sitter::Point;

    use crate::lsp::completions::request::SubsetOperator;
    use crate::lsp::completions::request::SubsetRequest;
    use crate::lsp::completions::sources::composite::subset::recognize_subset;
    use crate::lsp::document_context::TestDocument;

    fn subset_request_at(code: &str, point: Point) -> Option<SubsetRequest> {
        let doc = TestDocument::new(code);
        let context = doc.context(point);
        recognize_subset(&context).unwrap()
    }

    #[test]
    fn test_recognize_subset() {
        assert_eq!(
            subset_request_at("foo[]", Point { row: 0, column: 4 }),
            Some(SubsetRequest {
                object: String::from("foo"),
                operator: SubsetOperator::Single,
            })
        );

        assert_eq!(
            subset_request_at("foo[]", Point { row: 0, column: 3 }),
            None
        );

        assert_eq!(
            subset_request_at("foo[]", Point { row: 0, column: 5 }),
            None
        );

        assert_eq!(
            subset_request_at("x[, c()]", Point { row: 0, column: 6 }),
            Some(SubsetRequest {
                object: String::from("x"),
                operator: SubsetOperator::Single,
            })
        );

        assert_eq!(
            subset_request_at("x[[]]", Point { row: 0, column: 3 }),
            Some(SubsetRequest {
                object: String::from("x"),
                operator: SubsetOperator::Double,
            })
        );
    }
}
