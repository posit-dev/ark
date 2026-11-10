use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use oak_db::Db;
use oak_db::File;

/// An entry of the directory listed by [`file_path_completions()`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilePathItem {
    pub name: String,
    pub path: PathBuf,
    /// Whether the entry itself is a directory. A symbolic link to a
    /// directory is not.
    pub is_dir: bool,
}

/// List directory entries for a path typed in an R string in `file`.
/// `typed_path` is the string's decoded value up to the cursor, from
/// [`decode_string_prefix()`](oak_core::string::decode_string_prefix), not
/// its source text. This excludes text after the cursor and accepts a missing
/// closing quote. List `typed_path` if it is an existing directory, or its
/// parent otherwise. Results are sorted by name.
///
/// `~` and `~/...` expand to the home directory, but `~user` is left
/// unchanged. A relative path resolves against [`File::anchor_dir()`], or,
/// for a file without an anchor, against `fallback_dir`. The host supplies
/// this base directory for untitled documents and console input. Without
/// either base, a relative path lists nothing.
///
/// A directory that doesn't exist lists nothing, since strings that aren't
/// paths, such as URLs, are ordinary input. Other directory-listing errors
/// are returned. Unreadable entries are logged and skipped. If an entry
/// type cannot be read, the error is logged and `is_dir` is `false`.
pub fn file_path_completions(
    db: &dyn Db,
    file: File,
    typed_path: &str,
    fallback_dir: Option<&Path>,
) -> std::io::Result<Vec<FilePathItem>> {
    let mut path = expand_tilde(typed_path);
    if !path.has_root() {
        let base = match file.anchor_dir(db) {
            Some(anchor) => anchor.into_std_path_buf(),
            None => match fallback_dir {
                Some(fallback_dir) => fallback_dir.to_path_buf(),
                None => return Ok(Vec::new()),
            },
        };
        path = base.join(path);
    }

    if !path.is_dir() {
        if let Some(parent) = path.parent() {
            path = parent.to_path_buf();
        }
    }

    let entries = match std::fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };

    let mut items = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                log::error!("Can't read an entry of `{}`: {err:?}", path.display());
                continue;
            },
        };
        let is_dir = match entry.file_type() {
            Ok(file_type) => file_type.is_dir(),
            Err(err) => {
                log::error!(
                    "Can't read the type of `{}`: {err:?}",
                    entry.path().display()
                );
                false
            },
        };
        items.push(FilePathItem {
            name: entry.file_name().to_string_lossy().into_owned(),
            path: entry.path(),
            is_dir,
        });
    }

    items.sort_by(|lhs, rhs| lhs.name.cmp(&rhs.name));
    Ok(items)
}

/// Expand a leading `~` or `~/` to the home directory. `~user` and any other
/// path are returned unchanged, so a relative path stays relative for the
/// caller to resolve against the anchor or fallback directory.
fn expand_tilde(path: &str) -> PathBuf {
    let is_separator = |c: char| c == '/' || (cfg!(windows) && c == '\\');

    let rest = match path.strip_prefix('~') {
        Some("") => "",
        // Strip every leading separator: `~//x` is `$HOME//x` in R, and a
        // separator left at the start would make `join()` replace `home`
        Some(rest) if rest.starts_with(is_separator) => rest.trim_start_matches(is_separator),
        _ => return PathBuf::from(path),
    };

    let Some(home) = home_dir() else {
        return PathBuf::from(path);
    };
    home.join(rest)
}

/// On Windows, prefer `R_USER` when set, otherwise `HOME`. R sets `R_USER`
/// at startup, but a standalone host may not have it. If the selected value
/// is empty or unset, use [`home::home_dir()`], whose Windows fallback is
/// the profile directory rather than R's documents directory.
fn home_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        let home = std::env::var_os("R_USER").or_else(|| std::env::var_os("HOME"));
        if let Some(home) = home.filter(|home| !home.is_empty()) {
            return Some(PathBuf::from(home));
        }
    }
    home::home_dir().filter(|home| home != Path::new(""))
}
