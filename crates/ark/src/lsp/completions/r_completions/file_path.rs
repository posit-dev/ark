//
// file_path.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use std::env::current_dir;
use std::io::ErrorKind;
use std::path::PathBuf;

use harp::utils::r_is_string;
use harp::utils::r_normalize_path;
use stdext::unwrap;
use tower_lsp_server::ls_types::CompletionItem;

use crate::lsp::completions::completion_item::completion_item_from_direntry;
use crate::lsp::completions::sources::utils::set_sort_text_by_words_first;

/// Lists entries of the directory named by `token`, or its parent if the path
/// is not a directory. `token` includes quotes and escapes from the document.
pub(super) fn r_file_path_completions(token: &str) -> anyhow::Result<Vec<CompletionItem>> {
    log::trace!("r_file_path_completions()");

    let mut completions: Vec<CompletionItem> = vec![];

    // Parse the token to decode R string escapes before searching the directory.
    // Invalid strings, such as unrecognized backslash escapes, produce no
    // completions rather than a parse error (posit-dev/positron#6584).
    let Ok(contents) = harp::parse_expr(token) else {
        return Ok(completions);
    };

    // The token comes from a tree-sitter string node, but R's parser must also
    // recognize it as a string.
    if !r_is_string(contents.sexp) {
        return Ok(completions);
    }

    // Use R to normalize the path.
    let path = r_normalize_path(contents)?;

    // parse the file path and get the directory component
    let mut path = PathBuf::from(path.as_str());
    log::trace!("Normalized path: {}", path.display());

    // if this path doesn't have a root, add it on
    //
    // TODO: Align `source()` completions with `oak_db`'s `anchor_dir()`.
    // Using the session's working directory can suggest paths that oak cannot
    // resolve from the file's environment directory or workspace root.
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
    // A string naming no existing directory, such as a URL, is ordinary input
    // rather than a failure of this source
    let entries = match std::fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(completions),
        Err(err) => return Err(err.into()),
    };

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
    use crate::lsp::completions::r_completions::file_path::r_file_path_completions;
    use crate::r_task;

    #[test]
    fn test_unparseable_string() {
        // https://github.com/posit-dev/positron/issues/6584
        r_task(|| {
            // "\R" is an unrecognized escape character and `R_ParseVector()` errors on it
            let completions = r_file_path_completions(r#"".\R\utils.R""#).unwrap();
            assert_eq!(completions.len(), 0);
        })
    }

    #[test]
    fn test_missing_directory_is_not_an_error() {
        r_task(|| {
            let completions = r_file_path_completions("'https://posit.co/x'").unwrap();
            assert_eq!(completions.len(), 0);
        })
    }

    #[test]
    fn test_home_directory() {
        r_task(|| {
            // Assume home directory is not empty
            let completions = r_file_path_completions("'~/'").unwrap();
            assert!(!completions.is_empty());
        })
    }
}
