//! Determines a file's loader and the layers that loader makes visible.
//!

//! Does not read the file's semantic index, so [`File::cross_file_layers()`]
//! can call it while building that index.

pub(crate) mod contrib;

use camino::Utf8Path;

use crate::db::notebook_by_cell;
use crate::directory::collation_basename_key;
use crate::file_imports::CollationView;
use crate::File;
use crate::Package;
use crate::SourceDb;

/// Files, namespace imports, and packages supplied by a file's loader.
///
/// Source inheritance is lowered separately because it can add an
/// offset-narrowed [`ImportLayer::SourcingFile`].
pub(crate) struct LoadContext {
    pub kind: LoadKind,

    /// Visible files in lookup order, highest priority first, excluding the
    /// file itself.
    pub visible_files: Vec<File>,

    /// Number of leading `visible_files` that load after this file into the
    /// same environment. Deferred lookup assumes loading has finished, so
    /// these bindings shadow this file's top level, which shadows the rest.
    /// Files in parent environments, such as Shiny's `global.R` or a test's
    /// package namespace, rank below this file regardless of load order and
    /// do not count toward this prefix.
    pub later_sibling_count: usize,

    /// Packages attached by the loader, omitting packages unavailable in every
    /// root during lowering.
    pub implicit_attaches: Vec<&'static str>,

    /// Which loader produced this context. Resolution ignores it; diagnostics
    /// use it to name what already loads the file.
    pub loader: Option<LoaderInfo>,
}

/// How a loader names itself in user reports. Whichever module recognises the
/// loader supplies it, so a new one doesn't touch this file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LoaderInfo {
    /// Sentence subject, e.g. `"testthat"`.
    pub name: &'static str,

    /// Completes "<name> already loads <loads>".
    pub loads: &'static str,
}

const PACKAGE_LOADER: LoaderInfo = LoaderInfo {
    name: "The package",
    loads: "its `R/` files in collation order",
};

const NOTEBOOK_LOADER: LoaderInfo = LoaderInfo {
    name: "This notebook",
    loads: "its cells in document order",
};

/// The loader that owns `file`, if one does. Reads only paths and source text,
/// so it is safe to call while a semantic index is being built.
pub(crate) fn loader(db: &dyn SourceDb, file: File) -> Option<LoaderInfo> {
    load_context(db, file, CollationView::Deferred).loader
}

/// Resolver context selected by the loader.
///
/// Determines namespace imports, the search-path tail, and whether source-site
/// inheritance adds alternate lookup contexts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadKind {
    /// Resolve package imports from `package`'s NAMESPACE and end at `base`.
    Namespace(Package),

    /// Use the default session search path and allow source-site inheritance.
    Session,
}

impl LoadKind {
    /// Whether source-site inheritance is excluded because the loader fixes
    /// runtime evaluation order.
    pub fn fixes_load_order(self) -> bool {
        matches!(self, LoadKind::Namespace(_))
    }

    pub fn search_path_tail(self) -> SearchPathTail {
        match self {
            LoadKind::Namespace(_) => SearchPathTail::Base,
            LoadKind::Session => SearchPathTail::Default,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchPathTail {
    /// Only `base`. Package dependencies are supplied by the NAMESPACE.
    Base,

    Default,
}

/// Selects the first matching loader. Classifications overlap, so notebook
/// cells come first (a cell is never a package or app file), `testthat`
/// precedes package loading, and package ownership precedes directory
/// conventions.
pub(crate) fn load_context(db: &dyn SourceDb, file: File, view: CollationView) -> LoadContext {
    if let Some(context) = notebook_load_context(db, file, view) {
        return context;
    }
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

    standalone_load_context()
}

/// Uses document order to approximate notebook execution order because users
/// can run cells in any order, which is not known statically. Top-level code
/// sees preceding cells, while deferred code (function bodies) sees every
/// other cell. Lookup runs in reverse document order so later bindings shadow
/// earlier ones.
///
/// Applies to open Jupyter notebooks and Quarto / R Markdown documents, which
/// the editor presents as notebooks.
fn notebook_load_context(
    db: &dyn SourceDb,
    file: File,
    view: CollationView,
) -> Option<LoadContext> {
    let notebook = notebook_by_cell(db, file)?;
    let cells = notebook.cells(db);
    let prefix_len = cells.iter().position(|cell| *cell == file)?;

    let siblings = visible_siblings(file, cells, view, prefix_len);

    Some(LoadContext {
        kind: LoadKind::Session,
        later_sibling_count: siblings.later,
        visible_files: siblings.files,
        implicit_attaches: Vec::new(),
        loader: Some(NOTEBOOK_LOADER),
    })
}

/// A loadable `R/` file of a package, one of the files in `package.files()`.
///
/// Package membership alone does not make a file loadable. `data-raw/`, `inst/`,
/// and `R/` files omitted from `Collate:` are `package.scripts()` and remain
/// standalone.
fn package_load_context(db: &dyn SourceDb, file: File, view: CollationView) -> Option<LoadContext> {
    let package = file.package(db)?;
    let files = package.files(db);

    let prefix_len = files.iter().position(|sibling| *sibling == file)?;

    let siblings = visible_siblings(file, files, view, prefix_len);

    Some(LoadContext {
        kind: LoadKind::Namespace(package),
        later_sibling_count: siblings.later,
        visible_files: siblings.files,
        implicit_attaches: Vec::new(),
        loader: Some(PACKAGE_LOADER),
    })
}

/// A file nothing else loads. It sees only its own attaches and the search path.
fn standalone_load_context() -> LoadContext {
    LoadContext {
        kind: LoadKind::Session,
        visible_files: Vec::new(),
        later_sibling_count: 0,
        implicit_attaches: Vec::new(),
        loader: None,
    }
}

/// The `R/`-directory collation members visible to `file`, in LIFO order.
pub(crate) fn collation_visible_files(
    db: &dyn SourceDb,
    file: File,
    view: CollationView,
) -> VisibleSiblings {
    let files = file.collation_siblings(db);

    // Locate present files by identity because `A.R` and `a.R` share a sort
    // key but occupy distinct load positions. The key estimates the position
    // only for an orphan file the scanner has not placed yet.
    let prefix_len = match files.iter().position(|sibling| *sibling == file) {
        Some(position) => position,
        None => {
            let own_key = collation_basename_key(file, db);
            files.partition_point(|sibling| collation_basename_key(*sibling, db) < own_key)
        },
    };

    visible_siblings(file, files, view, prefix_len)
}

/// Siblings of `file` sharing its target environment, in reverse load order.
///
/// `files` excludes `file` to prevent `resolve()` from cycling through its
/// semantic index. `later` counts the leading entries that load after `file`.
/// Deferred lookup places the file's top-level bindings after that prefix
/// and before its predecessors. `Eager` includes only predecessors, so its
/// count is zero.
pub(crate) struct VisibleSiblings {
    pub files: Vec<File>,
    pub later: usize,
}

/// `prefix_len` is the file's position in runtime load order, or its insertion
/// position if the scanner has not placed it in `collation` yet. For an absent
/// file, deferred lookup counts every member at or after `prefix_len` as later.
pub(crate) fn visible_siblings(
    file: File,
    collation: &[File],
    view: CollationView,
    prefix_len: usize,
) -> VisibleSiblings {
    match view {
        CollationView::Eager => VisibleSiblings {
            files: collation[..prefix_len].iter().rev().copied().collect(),
            later: 0,
        },
        CollationView::Deferred => {
            let present = collation.get(prefix_len) == Some(&file);
            VisibleSiblings {
                later: collation.len() - prefix_len - usize::from(present),
                files: collation
                    .iter()
                    .rev()
                    .copied()
                    .filter(|sibling| *sibling != file)
                    .collect(),
            }
        },
    }
}

/// Whether `file` sits directly in an `R/` directory, which Shiny autoloads
/// alongside its app. The directory name is case-sensitive to match
/// [`load_context()`] and the package scanner.
pub(crate) fn in_r_directory(file: File, db: &dyn SourceDb) -> bool {
    let Some(path) = file.path(db).as_path() else {
        return false;
    };
    path.parent().and_then(Utf8Path::file_name) == Some("R")
}
