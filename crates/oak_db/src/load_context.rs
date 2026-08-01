//! Determines a file's loader and the layers that loader makes visible.
//!

//! Does not read the file's semantic index, so [`File::cross_file_layers()`]
//! can call it while building that index.

pub(crate) mod contrib;

use camino::Utf8Path;

use crate::directory::collation_basename_key;
use crate::file_imports::CollationView;
use crate::Db;
use crate::File;
use crate::Package;

/// Files, namespace imports, and packages supplied by a file's loader.
///
/// Source inheritance is lowered separately because it can add an
/// offset-narrowed [`ImportLayer::SourcingFile`].
pub(crate) struct LoadContext {
    /// Visible files in lookup order, highest priority first, excluding the
    /// file itself.
    pub visible_files: Vec<File>,

    pub namespace_owner: Option<Package>,

    /// Packages attached by the loader, omitting packages unavailable in every
    /// root during lowering.
    pub implicit_attaches: Vec<&'static str>,

    pub search_path_tail: SearchPathTail,

    /// Omit source-site inheritance when the loader fixes this file's runtime load order.
    pub fixed_load_order: bool,
}

pub(crate) enum SearchPathTail {
    /// Only `base`. Package dependencies are supplied by the NAMESPACE.
    Base,

    Default,
}

/// Selects the first matching loader. Classifications overlap, so `testthat`
/// precedes package loading and package ownership precedes directory
/// conventions.
pub(crate) fn load_context(db: &dyn Db, file: File, view: CollationView) -> LoadContext {
    if let Some(context) = contrib::testthat::load_context(db, file, view) {
        return context;
    }
    if let Some(context) = package_load_context(db, file, view) {
        return context;
    }

    // Shiny follows directory layout, so package ownership does not exclude an
    // app under `inst/app/`.
    if let Some(context) = contrib::shiny::load_context(db, file, view) {
        return context;
    }

    // Only unowned `R/` files use directory collation. A package file excluded
    // from `Collate:` has no loader and remains standalone.
    if file.package(db).is_none() {
        if let Some(context) = script_load_context(db, file, view) {
            return context;
        }
    }

    standalone_load_context()
}

/// A loadable `R/` file of a package, one of the files in `package.files()`.
///
/// Package membership alone does not make a file loadable. `data-raw/`, `inst/`,
/// and `R/` files omitted from `Collate:` are `package.scripts()` and remain
/// standalone.
fn package_load_context(db: &dyn Db, file: File, view: CollationView) -> Option<LoadContext> {
    let package = file.package(db)?;
    let files = package.files(db);

    let prefix_len = files.iter().position(|sibling| *sibling == file)?;

    Some(LoadContext {
        visible_files: visible_siblings(file, files, view, prefix_len),
        namespace_owner: Some(package),
        implicit_attaches: Vec::new(),
        search_path_tail: SearchPathTail::Base,
        fixed_load_order: true,
    })
}

/// A non-package script in an `R/` directory, collated alphabetically, exactly
/// like a package `R/` with no `Collate:`.
fn script_load_context(db: &dyn Db, file: File, view: CollationView) -> Option<LoadContext> {
    if !in_r_directory(file, db) {
        return None;
    }
    Some(LoadContext {
        visible_files: collation_visible_files(db, file, view),
        namespace_owner: None,
        implicit_attaches: Vec::new(),
        search_path_tail: SearchPathTail::Default,
        fixed_load_order: false,
    })
}

/// A file nothing else loads. It sees only its own attaches and the search path.
fn standalone_load_context() -> LoadContext {
    LoadContext {
        visible_files: Vec::new(),
        namespace_owner: None,
        implicit_attaches: Vec::new(),
        search_path_tail: SearchPathTail::Default,
        fixed_load_order: false,
    }
}

/// The `R/`-directory collation members visible to `file`, in LIFO order.
pub(crate) fn collation_visible_files(db: &dyn Db, file: File, view: CollationView) -> Vec<File> {
    let files = file.collation_siblings(db);

    // Before scanning moves `file` out of `OrphanRoot`, it is absent from this
    // list. Its sort key still locates the files loaded before it.
    let own_key = collation_basename_key(file, db);
    let prefix_len =
        files.partition_point(|sibling| collation_basename_key(*sibling, db) < own_key);

    visible_siblings(file, files, view, prefix_len)
}

/// Siblings visible to `file`, in reverse load order so later bindings shadow
/// earlier ones. Excludes `file` to prevent `resolve()` from cycling through its
/// semantic index. `Eager` keeps only its predecessors.
pub(crate) fn visible_siblings(
    file: File,
    collation: &[File],
    view: CollationView,
    prefix_len: usize,
) -> Vec<File> {
    match view {
        CollationView::Lazy => collation
            .iter()
            .rev()
            .copied()
            .filter(|sibling| *sibling != file)
            .collect(),
        CollationView::Eager => collation[..prefix_len].iter().rev().copied().collect(),
    }
}

/// Whether `file` sits directly in an `R/` directory, which triggers collation
/// for non-package scripts. The directory name is case-sensitive to match
/// [`load_context()`] and the package scanner.
pub(crate) fn in_r_directory(file: File, db: &dyn Db) -> bool {
    let Some(path) = file.path(db).as_path() else {
        return false;
    };
    path.parent().and_then(Utf8Path::file_name) == Some("R")
}
