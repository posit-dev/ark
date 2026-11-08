//
// extractor.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use harp::eval::RParseEvalOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::r_symbol;
use harp::utils::r_env_has;
use harp::utils::r_typeof;
use harp::Error;
use libr::STRSXP;
use tower_lsp_server::ls_types::CompletionItem;

use crate::console;
use crate::lsp::completions::completion_item::completion_item_from_data_variable;
use crate::lsp::completions::completion_item::QuoteStyle;
use crate::lsp::completions::request::ExtractOperator;
use crate::lsp::completions::sources::utils::set_sort_text_by_first_appearance;

/// Names of the evaluated `object` after `$` or `@`, from `.DollarNames()` or
/// `.AtNames()`.
pub(super) fn r_extractor_completions(
    object: &str,
    operator: ExtractOperator,
) -> anyhow::Result<Vec<CompletionItem>> {
    let fun = match operator {
        ExtractOperator::Dollar => ".DollarNames",
        ExtractOperator::At => ".AtNames",
    };

    log::trace!("r_extractor_completions({object:?}, {fun:?})");

    let mut completions = vec![];

    let env_utils = RFunction::new("base", "asNamespace").add("utils").call()?;
    let sym = r_symbol!(fun);

    if !r_env_has(*env_utils, sym) {
        // We'd like to generate these completions, but not a new enough version of R
        return Ok(completions);
    }

    let options = RParseEvalOptions {
        forbid_function_calls: true,
        env: console::selected_env(),
    };

    let evaluated = match harp::parse_eval(object, options) {
        Ok(evaluated) => evaluated,
        Err(err) => match err {
            // Function calls in the object expression are forbidden. The
            // recognized `$` or `@` context keeps an empty completion list.
            Error::UnsafeEvaluationError(_) => return Ok(completions),
            // LHS of the call evaluated to an error. Totally possible if the
            // user is writing pseudocode. Don't want to propagate an error here.
            _ => return Ok(completions),
        },
    };

    // Both `.DollarNames` and `.AtNames` have the same signature. Also, neither
    // provide a default value for `pattern` in the generic, but do provide a default
    // value of `pattern = ""` in the default S3 method. We manually pass through
    // `pattern = ""` in case we hit an S3 method which forgot to provide a default
    // value for `pattern` (like R6, posit-dev/positron#6699).
    let names = RFunction::new("utils", fun)
        .param("x", evaluated)
        .param("pattern", "")
        .call()?;

    if r_typeof(*names) != STRSXP {
        // Could come from a malformed user supplied S3 method
        return Ok(completions);
    }

    let names = names.to::<Vec<String>>()?;

    for name in names {
        match completion_item_from_data_variable(&name, object, QuoteStyle::BacktickIfNotSyntactic)
        {
            Ok(item) => completions.push(item),
            Err(err) => log::error!("{err:?}"),
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

    use crate::fixtures::package_is_installed;
    use crate::lsp::completions::r_completions::extractor::r_extractor_completions;
    use crate::lsp::completions::request::ExtractOperator;
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

            let completions = r_extractor_completions("foo", ExtractOperator::Dollar).unwrap();
            assert_eq!(completions.len(), 2);

            // Note, no sorting done!
            let completion = completions.first().unwrap();
            assert_eq!(completion.label, "b".to_string());

            let completion = completions.get(1).unwrap();
            assert_eq!(completion.label, "a".to_string());

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

            // No error and empty completions list
            // (If the user is typing pseudocode, we want to respect that and say that we
            // recognize they are on the RHS of a `$`, but respond with no completions
            // because they haven't specified a real object yet)
            let completions = r_extractor_completions("foo", ExtractOperator::Dollar).unwrap();
            assert_eq!(completions.len(), 0);
        })
    }

    #[test]
    fn test_dollar_completions_on_complex_lhs() {
        r_task(|| {
            // No error and empty completions list
            // We know we are on the RHS of a `$`, but `r_parse_eval()` will fail on the
            // LHS "object" because it is too complex, so the right thing to do is to
            // return an empty completion set to prevent other completion sources from
            // running.
            let completions =
                r_extractor_completions("list(a = 1, b = 2)", ExtractOperator::Dollar).unwrap();
            assert_eq!(completions.len(), 0);
        })
    }

    #[test]
    fn test_dollar_completions_on_r6() {
        r_task(|| {
            if !package_is_installed("R6") {
                return;
            }

            let options = RParseEvalOptions {
                forbid_function_calls: false,
                ..Default::default()
            };

            // Set up an R6 class object with a method named `abc`
            harp::parse_eval(
                r#"
Foo <- R6::R6Class('Foo', public = list(abc = function() {}))
foo <- Foo$new()
"#,
                options.clone(),
            )
            .unwrap();

            // We get some default R6 methods back, but we are looking for `abc`
            let completions = r_extractor_completions("foo", ExtractOperator::Dollar).unwrap();

            let completion_labels: Vec<String> = completions
                .into_iter()
                .map(|completion| completion.label)
                .collect();

            assert!(completion_labels.contains(&String::from("abc")));

            // Clean up
            harp::parse_eval("remove(foo, Foo)", options.clone()).unwrap();
        })
    }
}
