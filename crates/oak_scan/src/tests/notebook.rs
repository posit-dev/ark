use aether_path::FilePath;
use oak_db::DbInputs;
use oak_db::OakDatabase;
use url::Url;

use crate::inputs::DbScan;

fn cell_path(handle: usize) -> FilePath {
    FilePath::from_url(&Url::parse(&format!("vscode-notebook-cell:/nb.ipynb#W{handle}s")).unwrap())
}

fn notebook_path() -> FilePath {
    FilePath::parse("file:///nb.ipynb").unwrap()
}

#[test]
fn test_set_notebook_cells_reuses_the_notebook_entity() {
    let mut db = OakDatabase::new();
    let a = db.upsert_editor(cell_path(0), "a <- 1\n".to_string());
    let b = db.upsert_editor(cell_path(1), "b <- 1\n".to_string());

    let first = db.set_notebook_cells(notebook_path(), vec![a]);
    let second = db.set_notebook_cells(notebook_path(), vec![b, a]);

    assert_eq!(first, second);
    assert_eq!(second.cells(&db), &vec![b, a]);
    assert_eq!(db.open_notebooks().notebooks(&db), &vec![second]);
}

#[test]
fn test_close_notebook_removes_it() {
    let mut db = OakDatabase::new();
    let a = db.upsert_editor(cell_path(0), "a <- 1\n".to_string());
    db.set_notebook_cells(notebook_path(), vec![a]);

    db.close_notebook(&notebook_path());

    assert!(db.open_notebooks().notebooks(&db).is_empty());
}

#[test]
fn test_close_unknown_notebook_is_a_no_op() {
    let mut db = OakDatabase::new();

    db.close_notebook(&notebook_path());

    assert!(db.open_notebooks().notebooks(&db).is_empty());
}
