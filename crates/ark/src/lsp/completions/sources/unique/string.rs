//
// string.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use super::file_path::completions_from_file_path;
use crate::lsp::document_context::DocumentContext;

pub fn completions_from_string(context: &DocumentContext) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_string()");

    let node = context.node;

    if node.kind() != "string" {
        return Ok(None);
    }

    // Must actually be "inside" the string, so these places don't count, even
    // though they are detected as part of the string nodes `|""|`
    if node.start_position() == context.point || node.end_position() == context.point {
        return Ok(None);
    }

    // Even if we don't find any completions, we were inside a string so we
    // don't want to provide completions for anything else, so we always at
    // least return an empty `completions` vector from here.
    let mut completions: Vec<CompletionItem> = vec![];

    // Return empty set if we are here due to a trigger character like `$`.
    // See posit-dev/positron#1884.
    if context.trigger.is_some() {
        return Ok(Some(completions));
    }

    // Try file path completions
    completions.append(&mut completions_from_file_path(context)?);

    Ok(Some(completions))
}

#[cfg(test)]
mod tests {
    use harp::assert_match;
    use tree_sitter::Point;

    use crate::lsp::completions::sources::completions_from_unique_sources;
    use crate::lsp::completions::sources::unique::string::completions_from_string;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::lsp::traits::node::NodeExt;
    use crate::test::r_test;

    #[test]
    fn test_outside_quotes() {
        r_test(|| {
            // Before or after the `''`, i.e. `|''` or `''|`.
            // Still considered part of the string node.
            let point = Point { row: 0, column: 0 };
            let document = Document::new("''", None);
            let context = DocumentContext::new(&document, point, None);

            assert_eq!(context.node.kind(), "string");
            assert_eq!(completions_from_string(&context).unwrap(), None);
        })
    }

    #[test]
    fn test_not_string() {
        r_test(|| {
            let point = Point { row: 0, column: 0 };
            let document = Document::new("foo", None);
            let context = DocumentContext::new(&document, point, None);

            assert!(context.node.is_identifier());
            assert_eq!(completions_from_string(&context).unwrap(), None);
        })
    }

    #[test]
    fn test_trigger() {
        r_test(|| {
            // Before or after the `''`, i.e. `|''` or `''|`.
            // Still considered part of the string node.
            let point = Point { row: 0, column: 2 };

            // Assume home directory is not empty
            let document = Document::new("'~/'", None);

            // `None` trigger -> Return file completions
            let context = DocumentContext::new(&document, point, None);
            assert_match!(
                completions_from_string(&context).unwrap(),
                Some(items) => {
                    assert!(items.len() > 0)
                }
            );

            // `Some` trigger -> Should return empty completion set
            let context = DocumentContext::new(&document, point, Some(String::from("$")));
            let res = completions_from_string(&context).unwrap();
            assert_match!(res, Some(items) => { assert!(items.len() == 0) });

            // Check one level up too
            let res = completions_from_unique_sources(&context).unwrap();
            assert_match!(res, Some(items) => { assert!(items.len() == 0) });
        })
    }
}
