//
// string.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::Result;
use tower_lsp::lsp_types::CompletionItem;

use super::file_path::completions_from_string_file_path;
use crate::lsp::completions::sources::unique::subset::completions_from_string_subset;
use crate::lsp::document_context::DocumentContext;
use crate::treesitter::node_find_string;

pub fn completions_from_string(context: &DocumentContext) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_string()");

    let node = context.node;

    // Find actual `NodeType::String` node. Needed in case we are in its children.
    let Some(node) = node_find_string(&node) else {
        return Ok(None);
    };

    // Must actually be "inside" the string, so these places don't count, even
    // though they are detected as part of the string nodes `|""|`
    if node.start_position() == context.point || node.end_position() == context.point {
        return Ok(None);
    }

    // Even if we don't find any completions, we know we were inside a string so we
    // don't want to provide completions for anything else, so we always at
    // least return an empty `completions` vector from here on.
    let mut completions: Vec<CompletionItem> = vec![];

    // Return empty set if we are here due to a trigger character like `$`.
    // See posit-dev/positron#1884.
    if context.trigger.is_some() {
        return Ok(Some(completions));
    }

    // Check if we are doing string subsetting, like `x["<tab>"]`. This is a very unique
    // case that takes priority over file path completions.
    if let Some(mut candidates) = completions_from_string_subset(&node, context)? {
        completions.append(&mut candidates);
        return Ok(Some(completions));
    }

    // If no special string cases are hit, we show file path completions
    completions.append(&mut completions_from_string_file_path(&node, context)?);

    Ok(Some(completions))
}

#[cfg(test)]
mod tests {
    use stdext::assert_match;

    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::sources::completions_from_unique_sources;
    use crate::lsp::completions::sources::unique::string::completions_from_string;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::r_task;
    use crate::treesitter::node_find_string;
    use crate::treesitter::NodeTypeExt;

    #[test]
    fn test_outside_quotes() {
        r_task(|| {
            // Before or after the `''`, i.e. `|''` or `''|`.
            // Still considered part of the string node.
            let (text, point) = point_from_cursor("@''");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);

            assert!(node_find_string(&context.node).is_some());
            assert_eq!(completions_from_string(&context).unwrap(), None);
        })
    }

    #[test]
    fn test_not_string() {
        r_task(|| {
            let (text, point) = point_from_cursor("@foo");
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);

            assert!(context.node.is_identifier());
            assert_eq!(completions_from_string(&context).unwrap(), None);
        })
    }

    #[test]
    fn test_trigger() {
        r_task(|| {
            let (text, point) = point_from_cursor("'~/@'");

            // Assume home directory is not empty
            let document = Document::new(text.as_str(), None);

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
            let res = completions_from_unique_sources(&context, false).unwrap();
            assert_match!(res, Some(items) => { assert!(items.len() == 0) });
        })
    }
}
