use aether_path::FilePath;
use biome_rowan::TextSize;
use salsa::Setter;
use url::Url;

use crate::tests::file_imports::install_packages;
use crate::tests::file_imports::shape;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::Notebook;

/// Open a notebook whose R cells hold `sources`, in order. Returns the notebook
/// and its cells.
fn open_notebook(db: &mut TestDb, sources: &[&str]) -> (Notebook, Vec<File>) {
    let cells: Vec<File> = sources
        .iter()
        .enumerate()
        .map(|(handle, contents)| {
            let url = Url::parse(&format!("vscode-notebook-cell:/nb.ipynb#W{handle}s")).unwrap();
            File::new(
                db,
                FilePath::from_url(&url),
                FileRevision::zero(),
                Some(contents.to_string()),
                None,
            )
        })
        .collect();
    let notebook = Notebook::new(
        db,
        FilePath::parse("file:///nb.ipynb").unwrap(),
        cells.clone(),
    );
    db.open_notebooks().set_notebooks(db).to(vec![notebook]);
    (notebook, cells)
}

/// Byte offset of the last occurrence of `needle` in `cell`.
fn last_offset(db: &TestDb, cell: File, needle: &str) -> TextSize {
    TextSize::from(cell.source_text(db).rfind(needle).unwrap() as u32)
}

#[test]
fn test_notebook_cell_resolves_name_from_earlier_cell() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["helper <- function(a) a * 2\n", "helper(3)\n"]);

    let defs = cells[1].resolve_at(&db, last_offset(&db, cells[1], "helper"));

    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[0]);
}

#[test]
fn test_notebook_cell_top_level_does_not_see_later_cell() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["later\n", "later <- 1\n"]);

    let defs = cells[0].resolve_at(&db, last_offset(&db, cells[0], "later"));

    assert!(defs.is_empty());
}

#[test]
fn test_notebook_function_body_sees_later_cell() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["f <- function() later\n", "later <- 1\n"]);

    let defs = cells[0].resolve_at(&db, last_offset(&db, cells[0], "later"));

    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[1]);
}

#[test]
fn test_notebook_latest_earlier_cell_shadows() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["x <- 1\n", "x <- 2\n", "x\n"]);

    let defs = cells[2].resolve_at(&db, last_offset(&db, cells[2], "x"));

    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[1]);
}

#[test]
fn test_notebook_library_in_earlier_cell_attaches_for_later_cell() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "dplyr"]);
    let (_, cells) = open_notebook(&mut db, &["library(dplyr)\n", "mutate\n"]);

    // Same layer order as a package collation: the earlier cell, then its
    // attaches, then the default search path.
    assert_eq!(shape(&db, &cells[1].imports(&db)), vec![
        "File(nb.ipynb)".to_string(),
        "Package(dplyr)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_notebook_reorder_changes_visibility() {
    let mut db = TestDb::new();
    let (notebook, cells) = open_notebook(&mut db, &["y\n", "y <- 1\n"]);
    let use_offset = last_offset(&db, cells[0], "y");
    assert!(cells[0].resolve_at(&db, use_offset).is_empty());

    // Moving the definition above the use, as a `didChange` splice would.
    notebook.set_cells(&mut db).to(vec![cells[1], cells[0]]);

    let defs = cells[0].resolve_at(&db, use_offset);
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[1]);
}

#[test]
fn test_notebook_edit_in_earlier_cell_updates_resolution() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["old <- 1\n", "old\n"]);
    let use_offset = last_offset(&db, cells[1], "old");
    assert_eq!(cells[1].resolve_at(&db, use_offset).len(), 1);

    cells[0]
        .set_source_text_override(&mut db)
        .to(Some("new <- 1\n".to_string()));

    assert!(cells[1].resolve_at(&db, use_offset).is_empty());
}

#[test]
fn test_cell_outside_any_notebook_sees_no_other_cell() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["x <- 1\n", "x\n"]);
    db.open_notebooks().set_notebooks(&mut db).to(vec![]);

    let defs = cells[1].resolve_at(&db, last_offset(&db, cells[1], "x"));

    assert!(defs.is_empty());
}
