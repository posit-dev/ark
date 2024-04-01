//
// extractor.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use harp::eval::r_parse_eval;
use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::r_symbol;
use harp::utils::r_env_has;
use harp::utils::r_typeof;
use libr::STRSXP;
use tower_lsp::lsp_types::CompletionItem;

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

    let mut node = context.node;

    // If we are on the literal operator, look up one level to find the
    // parent. We have to do this because `DocumentContext` considers all
    // nodes, not just named ones.
    if matches!(node.node_type(), NodeType::Anonymous(operator) if matches!(operator.as_str(), "$" | "@"))
    {
        match node.parent() {
            Some(parent) => node = parent,
            None => return Ok(None),
        }
    }

    if node.node_type() != node_type {
        return Ok(None);
    }

    let mut completions: Vec<CompletionItem> = vec![];

    let Some(child) = node.child_by_field_name("lhs") else {
        return Ok(Some(completions));
    };

    let text = context.document.contents.node_slice(&child)?.to_string();

    completions.append(&mut completions_from_extractor_helper(text.as_str(), fun)?);

    Ok(Some(completions))
}

fn completions_from_extractor_helper(object: &str, fun: &str) -> Result<Vec<CompletionItem>> {
    log::info!("completions_from_extractor_helper({object:?}, {fun:?})");

    const ENQUOTE: bool = false;

    let mut completions = vec![];

    unsafe {
        let env_utils = RFunction::new("base", "asNamespace").add("utils").call()?;
        let sym = r_symbol!(fun);

        if !r_env_has(*env_utils, sym) {
            // We'd like to generate these completions, but not a new enough version of R
            return Ok(completions);
        }

        let value = r_parse_eval(object, RParseEvalOptions {
            forbid_function_calls: true,
            ..Default::default()
        })?;

        let names = RFunction::new("utils", fun).add(value).call()?;

        if r_typeof(*names) != STRSXP {
            // Could come from a malformed user supplied S3 method
            return Ok(completions);
        }

        let names = names.to::<Vec<String>>()?;

        for name in names {
            match completion_item_from_data_variable(&name, object, ENQUOTE) {
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
    use harp::eval::r_parse_eval;
    use harp::eval::RParseEvalOptions;
    use tree_sitter::Point;

    use crate::lsp::completions::sources::unique::extractor::completions_from_dollar;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::test::r_test;

    #[test]
    fn test_dollar_completions() {
        r_test(|| {
            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            // Set up a list with names
            r_parse_eval("foo <- list(b = 1, a = 2)", options.clone()).unwrap();

            // Right after the `$`
            let point = Point { row: 0, column: 4 };
            let document = Document::new("foo$", None);
            let context = DocumentContext::new(&document, point, None);

            let completions = completions_from_dollar(&context).unwrap().unwrap();
            assert_eq!(completions.len(), 2);

            // Note, no sorting done!
            let completion = completions.get(0).unwrap();
            assert_eq!(completion.label, "b".to_string());

            let completion = completions.get(1).unwrap();
            assert_eq!(completion.label, "a".to_string());

            // Right before the `$`
            let point = Point { row: 0, column: 3 };
            let document = Document::new("foo$", None);
            let context = DocumentContext::new(&document, point, None);
            let completions = completions_from_dollar(&context).unwrap();
            assert!(completions.is_none());

            // Clean up
            r_parse_eval("foo <- NULL", options.clone()).unwrap();
        })
    }
}
