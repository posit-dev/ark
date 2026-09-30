//! Notebook load context: a cell runs after the cells above it.
//!
//! Covers Jupyter notebooks and Quarto / R Markdown documents, which the
//! editor presents as notebooks. Reads only the [`crate::OpenNotebooks`]
//! input, not a semantic index, so [`File::cross_file_layers()`] can call it
//! while building that index.

use crate::db::notebook_by_cell;
use crate::file_imports::CollationView;
use crate::load_context::visible_siblings;
use crate::load_context::LoadContext;
use crate::load_context::LoadKind;
use crate::load_context::LoaderInfo;
use crate::File;
use crate::SourceDb;

const LOADER: LoaderInfo = LoaderInfo {
    name: "This notebook",
    loads: "its cells in document order",
};

/// A code cell of an open notebook. Top-level code sees the cells above it,
/// and deferred code (function bodies) sees every other cell: the same views a
/// package collation gives its `R/` files.
///
/// Document order stands in for execution order. The user can run cells in any
/// order, but that order is not known statically.
pub(crate) fn load_context(
    db: &dyn SourceDb,
    file: File,
    view: CollationView,
) -> Option<LoadContext> {
    let notebook = notebook_by_cell(db, file)?;
    let cells = notebook.cells(db);
    let prefix_len = cells.iter().position(|cell| *cell == file)?;

    Some(LoadContext {
        kind: LoadKind::Session,
        visible_files: visible_siblings(file, cells, view, prefix_len),
        implicit_attaches: Vec::new(),
        loader: Some(LOADER),
    })
}
