//
// file_path.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::env::current_dir;
use std::path::PathBuf;

use anyhow::Result;
use harp::object::RObject;
use harp::string::r_string_decode;
use harp::utils::r_normalize_path;
use stdext::unwrap;
use stdext::IntoResult;
use tower_lsp::lsp_types::CompletionItem;

use crate::lsp::completions::completion_item::completion_item_from_direntry;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;
use crate::lsp::document_context::DocumentContext;

pub fn completions_from_file_path(
    context: &DocumentContext,
) -> Result<Option<Vec<CompletionItem>>> {
    log::info!("completions_from_file_path()");

    let node = context.node;

    if node.kind() != "string" {
        return Ok(None);
    }

    // Must actually be "inside" the string, so these places don't count, even
    // though they are detected as part of the string nodes `|""|`
    if node.start_position() == context.point || node.end_position() == context.point {
        return Ok(None);
    }

    let mut completions: Vec<CompletionItem> = vec![];

    // Return empty set if we are here due to a trigger character like `$`.
    // See posit-dev/positron#1884.
    if context.trigger.is_some() {
        return Ok(Some(completions));
    }

    // Get the contents of the string token.
    //
    // NOTE: This includes the quotation characters on the string, and so
    // also includes any internal escapes! We need to decode the R string
    // before searching the path entries.
    let token = context.node.utf8_text(context.source.as_bytes())?;
    let contents = unsafe { r_string_decode(token).into_result()? };
    log::info!("String value (decoded): {}", contents);

    // Use R to normalize the path.
    let path = r_normalize_path(RObject::from(contents))?;

    // parse the file path and get the directory component
    let mut path = PathBuf::from(path.as_str());
    log::info!("Normalized path: {}", path.display());

    // if this path doesn't have a root, add it on
    if !path.has_root() {
        let root = current_dir()?;
        path = root.join(path);
    }

    // if this isn't a directory, get the parent path
    if !path.is_dir() {
        if let Some(parent) = path.parent() {
            path = parent.to_path_buf();
        }
    }

    // look for files in this directory
    log::info!("Reading directory: {}", path.display());
    let entries = std::fs::read_dir(path)?;
    for entry in entries.into_iter() {
        let entry = unwrap!(entry, Err(error) => {
            log::error!("{}", error);
            continue;
        });

        let item = unwrap!(completion_item_from_direntry(entry), Err(error) => {
            log::error!("{}", error);
            continue;
        });

        completions.push(item);
    }

    // Push path completions starting with non-word characters to the bottom of
    // the sort list (like those starting with `.`)
    set_sort_text_by_words_first(&mut completions);

    Ok(Some(completions))
}

#[cfg(test)]
mod tests {
    use harp::assert_match;
    use tree_sitter::Point;

    use crate::lsp::completions::sources::completions_from_unique_sources;
    use crate::lsp::completions::sources::unique::file_path::completions_from_file_path;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::test::r_test;

    #[test]
    fn test_file_path_outside_quotes() {
        r_test(|| {
            // Before or after the `''`, i.e. `|''` or `''|`.
            // Still considered part of the string node.
            let point = Point { row: 0, column: 0 };
            let document = Document::new("''");
            let context = DocumentContext::new(&document, point, None);

            assert_eq!(context.node.kind(), "string");
            assert_eq!(completions_from_file_path(&context).unwrap(), None);
        })
    }

    #[test]
    fn test_file_path_not_string() {
        r_test(|| {
            let point = Point { row: 0, column: 0 };
            let document = Document::new("foo");
            let context = DocumentContext::new(&document, point, None);

            assert_eq!(context.node.kind(), "identifier");
            assert_eq!(completions_from_file_path(&context).unwrap(), None);
        })
    }

    #[test]
    fn test_file_path_trigger() {
        r_test(|| {
            // Before or after the `''`, i.e. `|''` or `''|`.
            // Still considered part of the string node.
            let point = Point { row: 0, column: 2 };

            // Assume home directory is not empty
            let document = Document::new("'~/'");

            // `None` trigger -> Return file completions
            let context = DocumentContext::new(&document, point, None);
            assert_match!(
                completions_from_file_path(&context).unwrap(),
                Some(items) => {
                    assert!(items.len() > 0)
                }
            );

            // `Some` trigger -> Should return empty completion set
            let context = DocumentContext::new(&document, point, Some(String::from("$")));
            let res = completions_from_file_path(&context).unwrap();
            assert_match!(res, Some(items) => { assert!(items.len() == 0) });

            // Check one level up too
            let res = completions_from_unique_sources(&context).unwrap();
            assert_match!(res, Some(items) => { assert!(items.len() == 0) });
        })
    }
}
