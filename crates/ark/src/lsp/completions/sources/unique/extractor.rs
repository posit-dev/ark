//
// extractor.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::r_symbol;
use harp::utils::r_env_has;
use harp::utils::r_typeof;
use harp::Error;
use libr::STRSXP;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;

use crate::lsp::completions::completion_item::completion_item_from_data_variable;
use crate::lsp::completions::sources::utils::set_sort_text_by_first_appearance;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::rope::RopeExt;
use crate::treesitter::ExtractOperatorType;
use crate::treesitter::NodeType;
use crate::treesitter::NodeTypeExt;

pub fn completions_from_dollar(context: &DocumentContext) -> Result<Option<Vec<CompletionItem>>> {
    completions_from_extractor(
        context,
        NodeType::ExtractOperator(ExtractOperatorType::Dollar),
        ".DollarNames",
    )
}

pub fn completions_from_at(context: &DocumentContext) -> Result<Option<Vec<CompletionItem>>> {
    completions_from_extractor(
        context,
        NodeType::ExtractOperator(ExtractOperatorType::At),
        ".AtNames",
    )
}

fn completions_from_extractor(
    context: &DocumentContext,
    node_type: NodeType,
    fun: &str,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_extractor()");

    let node = context.node;

    let Some(node) = locate_extractor_node(node, node_type) else {
        // Not inside the RHS of an extractor node, let other completions run
        return Ok(None);
    };

    // At this point we know we are inside the RHS of a `$` or `@`, so from this point on
    // we either return an error or a "unique" set of completions, even if they are empty
    // (i.e. like if the object we evaluate doesn't exist because the user is typing
    // pseudocode or made a typo, or if it doesn't have names), to prevent any other
    // completion sources from running.
    let mut completions: Vec<CompletionItem> = vec![];

    // Get the object to evaluate that we collect completion names for
    let Some(node) = node.child_by_field_name("lhs") else {
        return Ok(Some(completions));
    };

    // Extract out its name from the document
    let text = context.document.contents.node_slice(&node)?.to_string();

    completions.append(&mut completions_from_extractor_object(text.as_str(), fun)?);

    Ok(Some(completions))
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
            return Some(parent);
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

fn completions_from_extractor_object(text: &str, fun: &str) -> Result<Vec<CompletionItem>> {
    log::info!("completions_from_extractor_object({text:?}, {fun:?})");

    const ENQUOTE: bool = false;

    let mut completions = vec![];

    unsafe {
        let env_utils = RFunction::new("base", "asNamespace").add("utils").call()?;
        let sym = r_symbol!(fun);

        if !r_env_has(*env_utils, sym) {
            // We'd like to generate these completions, but not a new enough version of R
            return Ok(completions);
        }

        let options = RParseEvalOptions {
            forbid_function_calls: true,
            ..Default::default()
        };

        let object = match harp::parse_eval(text, options) {
            Ok(object) => object,
            Err(err) => match err {
                // LHS of the call was too complex to evaluate. This is fine, we know
                // we are on the RHS of a `$` or `@`, so we return an empty "unique"
                // completion list to stop the completions search.
                Error::UnsafeEvaluationError(_) => return Ok(completions),
                // LHS of the call evaluated to an error. Totally possible if the
                // user is writing pseudocode. Don't want to propagate an error here.
                _ => return Ok(completions),
            },
        };

        let names = RFunction::new("utils", fun).add(object).call()?;

        if r_typeof(*names) != STRSXP {
            // Could come from a malformed user supplied S3 method
            return Ok(completions);
        }

        let names = names.to::<Vec<String>>()?;

        for name in names {
            match completion_item_from_data_variable(&name, text, ENQUOTE) {
                Ok(item) => completions.push(item),
                Err(err) => log::error!("{err:?}"),
            }
        }
    }

    // People typically expect that `$` and `@` completions are returned in
    // the same order as in the underlying object.
    set_sort_text_by_first_appearance(&mut completions);

    Ok(completions)
}

#[cfg(test)]
mod tests {
    use harp::eval::RParseEvalOptions;
    use harp::object::r_lgl_get;

    use crate::lsp::completions::sources::unique::extractor::completions_from_dollar;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::fixtures::point_from_cursor;
    use crate::r_task;

    #[test]
    fn test_dollar_completions() {
        r_task(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            // Set up a list with names
            harp::parse_eval("foo <- list(b = 1, a = 2)", options.clone()).unwrap();

            let (text, point) = point_from_cursor("foo$@");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);

            let completions = completions_from_dollar(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 2);

            // Note, no sorting done!
            let completion = completions.get(0).unwrap();
            assert_eq!(completion.label, "b".to_string());

            let completion = completions.get(1).unwrap();
            assert_eq!(completion.label, "a".to_string());

            let (text, point) = point_from_cursor("foo@$");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let completions = completions_from_dollar(&context).unwrap();
            assert!(completions.is_none());

            // Clean up
            harp::parse_eval("remove(foo)", options.clone()).unwrap();
        })
    }

    #[test]
    fn test_dollar_completions_on_nonexistent_object() {
        r_task(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            // `foo` should not exist in the environment `r_harp::parse_eval()` runs in
            let exists = harp::parse_eval("exists('foo')", options).unwrap();
            assert_eq!(r_lgl_get(exists.sexp, 0), 0);

            let (text, point) = point_from_cursor("foo$@");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);

            // No error and empty completions list
            // (If the user is typing pseudocode, we want to respect that and say that we
            // recognize they are on the RHS of a `$`, but respond with no completions
            // because they haven't specified a real object yet)
            let completions = completions_from_dollar(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 0);

            let (text, point) = point_from_cursor("foo$mat@");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);

            // Same as above
            let completions = completions_from_dollar(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 0);
        })
    }

    #[test]
    fn test_dollar_completions_on_complex_lhs() {
        r_task(|| {
            let (text, point) = point_from_cursor("list(a = 1, b = 2)$@");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);

            // No error and empty completions list
            // We know we are on the RHS of a `$`, but `r_parse_eval()` will fail on the
            // LHS "object" because it is too complex, so the right thing to do is to
            // return an empty completion set to prevent other completion sources from
            // running.
            let completions = completions_from_dollar(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 0);
        })
    }

    #[test]
    fn test_dollar_completions_before_the_dollar() {
        r_task(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            // Set up a list with names
            harp::parse_eval("foo <- list(b = 1, a = 2)", options.clone()).unwrap();

            let (text, point) = point_from_cursor("foo@$");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);

            // `None` because we have no completions to provide, and we do want other
            // completion sources to get a chance to run, as you can put arbitrary
            // expressions on the LHS of a `$` or `@`.
            let completions = completions_from_dollar(&context).unwrap();
            assert!(completions.is_none());

            // Clean up
            harp::parse_eval("remove(foo)", options.clone()).unwrap();
        })
    }

    #[test]
    fn test_dollar_completions_in_an_identifier() {
        r_task(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            // Set up a list with names
            harp::parse_eval("foo <- list(abcd = 1, wxyz = 2)", options.clone()).unwrap();

            let (text, point) = point_from_cursor("foo$abc@");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);

            // All names of `foo`, the frontend filters them
            let completions = completions_from_dollar(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 2);
            assert_eq!(completions.get(0).unwrap().label, String::from("abcd"));
            assert_eq!(completions.get(1).unwrap().label, String::from("wxyz"));

            let (text, point) = point_from_cursor("foo$a@bc");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);

            // Same as above
            let completions = completions_from_dollar(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 2);
            assert_eq!(completions.get(0).unwrap().label, String::from("abcd"));
            assert_eq!(completions.get(1).unwrap().label, String::from("wxyz"));

            // Clean up
            harp::parse_eval("remove(foo)", options.clone()).unwrap();
        })
    }
}
