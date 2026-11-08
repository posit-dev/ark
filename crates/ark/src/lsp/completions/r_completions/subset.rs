//
// subset.rs
//
// Copyright (C) 2024-2026 Posit Software, PBC. All rights reserved.
//
//

use tower_lsp_server::ls_types::CompletionItem;

use crate::lsp::completions::completion_item::QuoteStyle;
use crate::lsp::completions::r_completions::object_names::completions_from_evaluated_object_names;

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

    use crate::lsp::completions::r_completions::subset::r_string_subset_completions;
    use crate::lsp::completions::tests::utils::get_completions_at_cursor;
    use crate::r_task;

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
