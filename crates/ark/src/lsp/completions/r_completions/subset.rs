//
// subset.rs
//
// Copyright (C) 2024-2026 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp_server::ls_types::CompletionItem;

use crate::lsp::completions::completion_item::QuoteStyle;
use crate::lsp::completions::r_completions::object_names::completions_from_evaluated_object_names;
use crate::lsp::completions::request::SubsetRequest;

/// Names of the evaluated `object` inside `x[|]` or `x[[|]]`, where `|` marks
/// the cursor. Names are double quoted except with `data.table`'s `[`.
pub(super) fn r_subset_completions(request: &SubsetRequest) -> anyhow::Result<Vec<CompletionItem>> {
    log::trace!("r_subset_completions()");

    let SubsetRequest { object, operator } = request;
    let completions =
        completions_from_evaluated_object_names(object, QuoteStyle::Double, Some(*operator))?;
    Ok(completions.unwrap_or_default())
}

/// Names of the evaluated `object` inside `x["|"]`, without added quotes
/// because they are inserted into the string under the cursor.
pub(super) fn r_string_subset_completions(object: &str) -> anyhow::Result<Vec<CompletionItem>> {
    log::trace!("r_string_subset_completions()");

    // `None` disables `data.table`'s `[` quoting rule. Any added backticks
    // would become part of the string value.
    let completions = completions_from_evaluated_object_names(object, QuoteStyle::None, None)?;
    Ok(completions.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use harp::eval::parse_eval_global;
    use tower_lsp_server::ls_types::CompletionItem;

    use crate::fixtures::package_is_installed;
    use crate::lsp::completions::r_completions::subset::r_string_subset_completions;
    use crate::lsp::completions::r_completions::subset::r_subset_completions;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::lsp::completions::tests::utils::request_at_cursor;
    use crate::r_task;

    fn subset_completions_at_cursor(code_with_cursor: &str) -> Vec<CompletionItem> {
        let request = request_at_cursor(code_with_cursor)
            .composite
            .subset
            .unwrap();
        r_subset_completions(&request).unwrap()
    }

    #[test]
    fn test_subset_completions() {
        r_task(|| {
            parse_eval_global("foo <- list(b = 1, a = 2)").unwrap();

            let completions = subset_completions_at_cursor("foo[@]");
            assert_eq!(completions.len(), 2);

            let completion = completions.first().unwrap();
            assert_eq!(completion.label, "b".to_string());
            assert_eq!(completion.insert_text, Some("\"b\"".to_string()));

            let completion = completions.get(1).unwrap();
            assert_eq!(completion.label, "a".to_string());
            assert_eq!(completion.insert_text, Some("\"a\"".to_string()));

            parse_eval_global("remove(foo)").unwrap();
        })
    }

    #[test]
    fn test_data_table_subset_completions() {
        r_task(|| {
            if !package_is_installed("data.table") {
                return;
            }

            parse_eval_global("x <- data.table::as.data.table(mtcars)").unwrap();

            let completions = subset_completions_at_cursor("x[@]");
            assert_eq!(completions.len(), 11);

            let completion = completions.first().unwrap();
            assert_eq!(completion.label, "mpg".to_string());
            assert_eq!(completion.insert_text, None);

            let completions = subset_completions_at_cursor("x[, c(@)]");
            assert_eq!(completions.len(), 11);

            let completion = completions.first().unwrap();
            assert_eq!(completion.label, "mpg".to_string());
            assert_eq!(completion.insert_text, None);

            // A selected column does not suppress the other column completions.
            let completions = subset_completions_at_cursor("x[, c(mpg,@)]");
            assert_eq!(completions.len(), 11);

            let completion = completions.first().unwrap();
            // TODO: Exclude the selected `mpg` column or rank it after other columns.
            assert_eq!(completion.label, "mpg".to_string());
            assert_eq!(completion.insert_text, None);

            let completions = subset_completions_at_cursor("x[[@]]");
            assert_eq!(completions.len(), 11);

            let completion = completions.first().unwrap();
            assert_eq!(completion.label, "mpg".to_string());
            assert_eq!(completion.insert_text, Some("\"mpg\"".to_string()));

            parse_eval_global("remove(x)").unwrap();
        })
    }

    #[test]
    fn test_string_subset_completions() {
        r_task(|| {
            // Set up a list with names
            parse_eval_global("foo <- list(b = 1, a = 2)").unwrap();

            let completions = r_string_subset_completions("foo").unwrap();
            assert_eq!(completions.len(), 2);

            let completion = completions.first().unwrap();
            assert_eq!(completion.label, "b".to_string());
            // Not enquoting, so uses `label` directly
            assert_eq!(completion.insert_text, None);

            let completion = completions.get(1).unwrap();
            assert_eq!(completion.label, "a".to_string());
            // Not enquoting, so uses `label` directly
            assert_eq!(completion.insert_text, None);

            // `c()` string subsetting also works through both completion passes.
            let completions = get_completions_at_cursor(r#"foo[c("a", "@")]"#).unwrap();
            assert_eq!(completions.len(), 2);

            // A fake object that we can't get object names for.
            // It _looks_ like we want string completions though, so we return an empty set.
            let completions = r_string_subset_completions("not_real").unwrap();
            assert!(completions.is_empty());

            // Clean up
            parse_eval_global("remove(foo)").unwrap();
        })
    }

    #[test]
    fn test_string_subset_completions_on_matrix() {
        r_task(|| {
            // Set up a list with names
            parse_eval_global("foo <- array(1, dim = c(2, 2))").unwrap();
            parse_eval_global("colnames(foo) <- c('a', 'b')").unwrap();

            let completions = r_string_subset_completions("foo").unwrap();
            assert_eq!(completions.len(), 2);
            assert_eq!(completions.first().unwrap().label, "a".to_string());
            assert_eq!(completions.get(1).unwrap().label, "b".to_string());

            // Clean up
            parse_eval_global("remove(foo)").unwrap();
        })
    }

    #[test]
    fn test_string_subset_completions_with_non_syntactic_names() {
        r_task(|| {
            // Set up a list with non syntactic name
            parse_eval_global("x <- list(`_a` = 1)").unwrap();

            let completions = r_string_subset_completions("x").unwrap();
            assert_eq!(completions.len(), 1);

            // Note how `_a` isn't backticked! We know we are inserting it directly
            // into existing double quotes (posit-dev/positron#14909).
            assert_eq!(completions.first().unwrap().label, "_a".to_string());
        })
    }
}
