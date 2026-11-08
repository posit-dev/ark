//
// extractor.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use tree_sitter::Node;

use crate::lsp::completions::request::ExtractOperator;
use crate::lsp::completions::request::UniqueRequest;
use crate::lsp::completions::sources::unique::Recognized;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::node::NodeExt;
use crate::treesitter::ExtractOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

pub(super) fn recognize_dollar(context: &DocumentContext) -> anyhow::Result<Option<Recognized>> {
    recognize_extractor(context, ExtractOperator::Dollar)
}

pub(super) fn recognize_at(context: &DocumentContext) -> anyhow::Result<Option<Recognized>> {
    recognize_extractor(context, ExtractOperator::At)
}

fn recognize_extractor(
    context: &DocumentContext,
    operator: ExtractOperator,
) -> anyhow::Result<Option<Recognized>> {
    let node_type = match operator {
        ExtractOperator::Dollar => NodeType::ExtractOperator(ExtractOperatorType::Dollar),
        ExtractOperator::At => NodeType::ExtractOperator(ExtractOperatorType::At),
    };

    let Some(node) = locate_extractor_node(context.node, node_type) else {
        // Not inside the RHS of an extractor node, let other completions run
        return Ok(None);
    };

    // On the RHS of `$` or `@`, suppress later sources even if the object has
    // no names, cannot be evaluated, or the session pass does not run.

    // Get the object to evaluate that we collect completion names for
    let Some(node) = node.child_by_field_name("lhs") else {
        return Ok(Some(Recognized::Empty));
    };

    // Extract out its name from the document
    let object = node.node_as_str(context.contents)?.to_string();

    Ok(Some(Recognized::Session(UniqueRequest::Extractor {
        object,
        operator,
    })))
}

fn locate_extractor_node(node: Node, node_type: NodeType) -> Option<Node> {
    // `DocumentContext` considers all nodes, not just named ones, so we will have
    // drilled down into either the LHS, RHS, or the anonymous `$` or `@` node by now.
    let parent = node.parent()?;

    if parent.node_type() != node_type {
        // Parent node isn't an extractor node type, nothing to do
        return None;
    }

    match node.node_type() {
        NodeType::Anonymous(operator) if matches!(operator.as_str(), "$" | "@") => {
            // Cursor should be on the RHS of the `operator`
            Some(parent)
        },
        NodeType::Identifier => {
            // Only provide completions for the RHS child
            if node == parent.child_by_field_name("rhs")? {
                Some(parent)
            } else {
                None
            }
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::fixtures::point_and_offset_from_cursor;
    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::request::ExtractOperator;
    use crate::lsp::completions::request::UniqueRequest;
    use crate::lsp::completions::sources::unique::extractor::recognize_at;
    use crate::lsp::completions::sources::unique::extractor::recognize_dollar;
    use crate::lsp::completions::sources::unique::Recognized;
    use crate::lsp::document_context::TestDocument;

    fn dollar_at_cursor(code: &str) -> Option<Recognized> {
        let (text, point) = point_from_cursor(code);
        let doc = TestDocument::new(&text);
        let context = doc.context(point);
        recognize_dollar(&context).unwrap()
    }

    fn extractor_slot(object: &str, operator: ExtractOperator) -> Option<Recognized> {
        Some(Recognized::Session(UniqueRequest::Extractor {
            object: String::from(object),
            operator,
        }))
    }

    #[test]
    fn test_dollar_slot() {
        assert_eq!(
            dollar_at_cursor("foo$@"),
            extractor_slot("foo", ExtractOperator::Dollar)
        );

        // Function calls are recognized here but rejected by session evaluation.
        assert_eq!(
            dollar_at_cursor("list(a = 1, b = 2)$@"),
            extractor_slot("list(a = 1, b = 2)", ExtractOperator::Dollar)
        );

        // The frontend filters names by the identifier under the cursor.
        assert_eq!(
            dollar_at_cursor("foo$abc@"),
            extractor_slot("foo", ExtractOperator::Dollar)
        );
        assert_eq!(
            dollar_at_cursor("foo$a@bc"),
            extractor_slot("foo", ExtractOperator::Dollar)
        );
    }

    #[test]
    fn test_dollar_slot_before_the_dollar() {
        // `None` because we have no completions to provide, and we do want other
        // completion sources to get a chance to run, as you can put arbitrary
        // expressions on the LHS of a `$` or `@`.
        assert_eq!(dollar_at_cursor("foo@$"), None);
    }

    #[test]
    fn test_at_slot() {
        // `@` is the default cursor marker.
        let (text, point, _) = point_and_offset_from_cursor("foo@|", b'|');
        let doc = TestDocument::new(&text);
        let context = doc.context(point);

        assert_eq!(
            recognize_at(&context).unwrap(),
            extractor_slot("foo", ExtractOperator::At)
        );
        assert_eq!(recognize_dollar(&context).unwrap(), None);
    }
}
