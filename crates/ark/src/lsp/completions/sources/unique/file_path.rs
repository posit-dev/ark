//
// file_path.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

use std::env::current_dir;
use std::path::PathBuf;

use harp::utils::r_is_string;
use harp::utils::r_normalize_path;
use stdext::unwrap;
use tower_lsp::lsp_types::CompletionItem;
use tree_sitter::Node;

use crate::lsp::completions::completion_item::completion_item_from_direntry;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;
use crate::lsp::document_context::DocumentContext;
use crate::lsp::traits::rope::RopeExt;

pub(super) fn completions_from_string_file_path(
    node: &Node,
    context: &DocumentContext,
) -> anyhow::Result<Vec<CompletionItem>> {
    log::trace!("completions_from_string_file_path()");

    let mut completions: Vec<CompletionItem> = vec![];

    // Get the contents of the string token.
    //
    // NOTE: This includes the quotation characters on the string, and so
    // also includes any internal escapes! We need to decode the R string
    // by parsing it before searching the path entries.
    let token = context.document.contents.node_slice(&node)?.to_string();

    // It's entirely possible that we can fail to parse the string, `R_ParseVector()`
    // can fail in various ways. We silently swallow these because they are unlikely
    // to report to real file paths and just bail (posit-dev/positron#6584).
    let Ok(contents) = harp::parse_expr(&token) else {
        return Ok(completions);
    };

    // Double check that parsing gave a string. It should, because `node` points to
    // a tree-sitter string node.
    if !r_is_string(contents.sexp) {
        return Ok(completions);
    }

    // Use R to normalize the path.
    let path = r_normalize_path(contents)?;

    // parse the file path and get the directory component
    let mut path = PathBuf::from(path.as_str());
    log::trace!("Normalized path: {}", path.display());

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
    log::trace!("Reading directory: {}", path.display());
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

    Ok(completions)
}

#[cfg(test)]
mod tests {
    use crate::fixtures::point_from_cursor;
    use crate::lsp::completions::sources::unique::file_path::completions_from_string_file_path;
    use crate::lsp::document_context::DocumentContext;
    use crate::lsp::documents::Document;
    use crate::r_task;
    use crate::treesitter::node_find_string;

    #[test]
    fn test_unparseable_string() {
        // https://github.com/posit-dev/positron/issues/6584
        r_task(|| {
            // "\R" is an unrecognized escape character and `R_ParseVector()` errors on it
            let (text, point) = point_from_cursor(r#" ".\R\utils.R@" "#);
            let document = Document::new(text.as_str(), None);
            let context = DocumentContext::new(&document, point, None);
            let node = node_find_string(&context.node).unwrap();

            let completions = completions_from_string_file_path(&node, &context).unwrap();
            assert_eq!(completions.len(), 0);
        })
    }
}
