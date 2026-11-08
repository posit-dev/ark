//
// object_names.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use harp::error::Error;
use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::utils::r_inherits;
use tower_lsp_server::ls_types::CompletionItem;

use crate::console;
use crate::lsp::completions::completion_item::completion_item_from_data_variable;
use crate::lsp::completions::completion_item::QuoteStyle;
use crate::lsp::completions::request::SubsetOperator;

/// Names of the object that `name` evaluates to, or `None` if it can't be
/// evaluated.
///
/// For `data.table`, `Some(SubsetOperator::Single)` uses backticks rather than
/// double quotes for non-syntactic names in `[`'s non-standard evaluation.
/// Pass `None` inside a string to preserve the requested `quote_style`.
pub(crate) fn completions_from_evaluated_object_names(
    name: &str,
    quote_style: QuoteStyle,
    operator: Option<SubsetOperator>,
) -> anyhow::Result<Option<Vec<CompletionItem>>> {
    log::trace!("completions_from_evaluated_object_names({name:?})");

    let options = RParseEvalOptions {
        forbid_function_calls: true,
        env: console::selected_env(),
    };

    // Try to evaluate the object
    let object = harp::parse_eval(name, options);

    // If we get an `UnsafeEvaluationError` here from setting
    // `forbid_function_calls`, we don't even log that one, as that is
    // expected to happen with complex inputs.
    // If we get a `TryCatchError`, that is typically an 'object not found' error resulting
    // from the user typing pseudocode. Log those at info level without a full backtrace.
    let object = match object {
        Ok(object) => object,
        Err(err) => match err {
            Error::UnsafeEvaluationError(_) => return Ok(None),
            Error::TryCatchError(err) => {
                log::trace!("Can't evaluate object: {}", err.message);
                return Ok(None);
            },
            _ => {
                log::error!("Can't evaluate object: {err}");
                return Ok(None);
            },
        },
    };

    let completions = if harp::utils::r_is_matrix(object.sexp) {
        // Special case just for 2D arrays
        completions_from_object_colnames(object, name, quote_style)?
    } else if r_inherits(object.sexp, "data.table") {
        // The `[` method for data.table uses NSE so we don't double quote the names
        // https://github.com/posit-dev/positron/issues/3140
        let quote_style = if operator == Some(SubsetOperator::Single) {
            QuoteStyle::BacktickIfNotSyntactic
        } else {
            quote_style
        };
        completions_from_object_names(object, name, quote_style)?
    } else {
        completions_from_object_names(object, name, quote_style)?
    };

    Ok(Some(completions))
}

pub(crate) fn completions_from_object_names(
    object: RObject,
    name: &str,
    quote_style: QuoteStyle,
) -> anyhow::Result<Vec<CompletionItem>> {
    completions_from_object_names_impl(object, name, quote_style, "names")
}

fn completions_from_object_colnames(
    object: RObject,
    name: &str,
    quote_style: QuoteStyle,
) -> anyhow::Result<Vec<CompletionItem>> {
    completions_from_object_names_impl(object, name, quote_style, "colnames")
}

fn completions_from_object_names_impl(
    object: RObject,
    name: &str,
    quote_style: QuoteStyle,
    function: &str,
) -> anyhow::Result<Vec<CompletionItem>> {
    log::trace!("completions_from_object_names_impl({object:?})");

    let mut completions = vec![];

    let element_names = RFunction::new("base", function)
        .add(object)
        .call()?
        .to::<Vec<String>>()?;

    for element_name in element_names {
        match completion_item_from_data_variable(&element_name, name, quote_style) {
            Ok(item) => completions.push(item),
            Err(err) => log::error!("{err:?}"),
        }
    }

    Ok(completions)
}

#[cfg(test)]
mod tests {
    use harp::eval::parse_eval_global;

    use crate::fixtures::package_is_installed;
    use crate::lsp::completions::completion_item::QuoteStyle;
    use crate::lsp::completions::r_completions::object_names::completions_from_evaluated_object_names;
    use crate::lsp::completions::request::SubsetOperator;
    use crate::r_task;

    #[test]
    fn test_completions_from_evaluated_object_names() {
        r_task(|| {
            let operator = Some(SubsetOperator::Single);

            // Vector with names
            parse_eval_global("x <- 1:2").unwrap();
            parse_eval_global("names(x) <- c('a', 'b')").unwrap();

            let completions =
                completions_from_evaluated_object_names("x", QuoteStyle::Double, operator)
                    .unwrap()
                    .unwrap();
            assert_eq!(completions.len(), 2);
            assert_eq!(completions.first().unwrap().label, String::from("a"));
            assert_eq!(completions.get(1).unwrap().label, String::from("b"));

            parse_eval_global("remove(x)").unwrap();

            // Data frame
            parse_eval_global("x <- data.frame(a = 1, b = 2, c = 3)").unwrap();

            let completions =
                completions_from_evaluated_object_names("x", QuoteStyle::Double, operator)
                    .unwrap()
                    .unwrap();
            assert_eq!(completions.len(), 3);
            assert_eq!(completions.first().unwrap().label, String::from("a"));
            assert_eq!(completions.get(1).unwrap().label, String::from("b"));
            assert_eq!(completions.get(2).unwrap().label, String::from("c"));

            parse_eval_global("remove(x)").unwrap();

            // 1D array with names
            parse_eval_global("x <- array(1:2)").unwrap();
            parse_eval_global("names(x) <- c('a', 'b')").unwrap();

            let completions =
                completions_from_evaluated_object_names("x", QuoteStyle::Double, operator)
                    .unwrap()
                    .unwrap();
            assert_eq!(completions.len(), 2);
            assert_eq!(completions.first().unwrap().label, String::from("a"));
            assert_eq!(completions.get(1).unwrap().label, String::from("b"));

            parse_eval_global("remove(x)").unwrap();

            // Matrix with column names
            parse_eval_global("x <- array(1, dim = c(1, 1))").unwrap();
            parse_eval_global("rownames(x) <- 'a'").unwrap();
            parse_eval_global("colnames(x) <- 'b'").unwrap();

            let completions =
                completions_from_evaluated_object_names("x", QuoteStyle::Double, operator)
                    .unwrap()
                    .unwrap();
            assert_eq!(completions.len(), 1);
            assert_eq!(completions.first().unwrap().label, String::from("b"));

            parse_eval_global("remove(x)").unwrap();

            // 3D array with column names
            // We currently decide not to return any names here. It is typically quite
            // ambiguous which axis's names you'd want when working with >=3D arrays.
            // But we did find an object, so we return an empty vector.
            parse_eval_global("x <- array(1, dim = c(1, 1, 1))").unwrap();
            parse_eval_global("rownames(x) <- 'a'").unwrap();
            parse_eval_global("colnames(x) <- 'b'").unwrap();

            let completions =
                completions_from_evaluated_object_names("x", QuoteStyle::Double, operator)
                    .unwrap()
                    .unwrap();
            assert!(completions.is_empty());

            parse_eval_global("remove(x)").unwrap();
        })
    }

    #[test]
    fn test_data_table_completions() {
        r_task(|| {
            // Skip test if data.table is not installed
            if !package_is_installed("data.table") {
                return;
            }

            parse_eval_global("x <- data.table::as.data.table(mtcars)").unwrap();

            // Subset completions
            // This is where we have an override for data.table in particular, which
            // changes our provided `QuoteStyle::Double` to
            // `QuoteStyle::BacktickIfNotSyntactic` due to data.table's NSE in `[`.
            let completions = completions_from_evaluated_object_names(
                "x",
                QuoteStyle::Double,
                Some(SubsetOperator::Single),
            )
            .unwrap()
            .unwrap();

            assert_eq!(completions.len(), 11);
            assert_eq!(completions.first().unwrap().label, String::from("mpg"));
            assert_eq!(completions.first().unwrap().insert_text, None);

            // Subset2 completions
            let completions = completions_from_evaluated_object_names(
                "x",
                QuoteStyle::Double,
                Some(SubsetOperator::Double),
            )
            .unwrap()
            .unwrap();

            assert_eq!(completions.len(), 11);
            assert_eq!(completions.first().unwrap().label, String::from("mpg"));
            assert_eq!(
                completions.first().unwrap().insert_text,
                Some("\"mpg\"".to_string())
            );

            parse_eval_global("remove(x)").unwrap();
        })
    }
}
