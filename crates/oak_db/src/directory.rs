//! Path-only directory listings for workspace-root files.
//!
//! Package-owned files are included so conventions resolve `inst/app/` like
//! loose workspace directories.
//!
//! These helpers don't call into semantic indexes, so load-context discovery
//! can call them while an index is being built.

use camino::Utf8Path;

use crate::db::workspace_root_files;
use crate::Db;
use crate::File;

/// Returns workspace files directly under `dir`, in `list.files()` load order.
///
/// `sourceDir()`, Shiny's `loadSupport()`, and non-package `R/` collation use
/// this order. [`collation_basename_key()`] mirrors their session-locale sort.
pub(crate) fn files_in_directory(db: &dyn Db, dir: &Utf8Path) -> Vec<File> {
    let mut files: Vec<File> = workspace_root_files(db)
        .iter()
        .copied()
        .filter(|file| file.path(db).as_path().and_then(Utf8Path::parent) == Some(dir))
        .collect();

    files.sort_by_cached_key(|file| collation_basename_key(*file, db));
    files
}

/// Returns workspace files below `dir` in `targets::tar_source()` load order.
///
/// `list.files(recursive = TRUE)` sorts nested scripts by relative path. Case
/// folding matches [`collation_basename_key()`].
pub(crate) fn files_in_directory_recursive(db: &dyn Db, dir: &Utf8Path) -> Vec<File> {
    let mut keyed: Vec<(String, File)> = workspace_root_files(db)
        .iter()
        .copied()
        .filter_map(|file| {
            let relative = file.path(db).as_path()?.strip_prefix(dir).ok()?;
            Some((relative.as_str().to_ascii_lowercase(), file))
        })
        .collect();

    keyed.sort_by(|(left, _), (right, _)| left.cmp(right));
    keyed.into_iter().map(|(_, file)| file).collect()
}

/// ASCII-case-folded basename key approximating `list.files()` session-locale
/// ordering.
///
/// Package installation instead forces `LC_COLLATE=C`, where raw byte order
/// determines collation.
pub(crate) fn collation_basename_key(file: File, db: &dyn Db) -> Option<String> {
    file.path(db)
        .file_name()
        .map(|name| name.to_ascii_lowercase())
}
