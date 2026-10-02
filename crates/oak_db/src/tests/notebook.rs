use aether_path::FilePath;
use biome_rowan::TextSize;
use salsa::Setter;
use url::Url;

use crate::tests::file_imports::install_packages;
use crate::tests::file_imports::shape;
use crate::tests::test_db::file_path;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::Name;
use crate::Notebook;

fn open_notebook(db: &mut TestDb, sources: &[&str]) -> (Notebook, Vec<File>) {
    add_notebook(db, "nb", sources)
}

fn add_notebook(db: &mut TestDb, name: &str, sources: &[&str]) -> (Notebook, Vec<File>) {
    let cells: Vec<File> = sources
        .iter()
        .enumerate()
        .map(|(handle, contents)| {
            let url =
                Url::parse(&format!("vscode-notebook-cell:/{name}.ipynb#W{handle}s")).unwrap();
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
        FilePath::parse(&format!("file:///{name}.ipynb")).unwrap(),
        cells.clone(),
    );
    let mut notebooks = db.open_notebooks().notebooks(db).clone();
    notebooks.push(notebook);
    db.open_notebooks().set_notebooks(db).to(notebooks);
    (notebook, cells)
}

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

    assert_eq!(shape(&db, cells[1].imports(&db)), vec![
        "File(nb.ipynb)".to_string(),
        "Package(dplyr)".to_string(),
        "Package(base)".to_string(),
    ]);
}

#[test]
fn test_notebook_loader_fallback_excludes_cells_and_narrows_own_attaches() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "dplyr", "rlang"]);
    let (_, cells) = open_notebook(&mut db, &[
        "library(dplyr)\n",
        "before\nlibrary(rlang)\nafter\n",
    ]);
    let cell = cells[1];
    let before = cell.loader_fallback_at(&db, last_offset(&db, cell, "before"));
    assert_eq!(shape(&db, &before), vec!["Package(dplyr)", "Package(base)"]);
    let after = cell.loader_fallback_at(&db, last_offset(&db, cell, "after"));
    assert_eq!(shape(&db, &after), vec![
        "Package(rlang)",
        "Package(dplyr)",
        "Package(base)"
    ]);
    assert_eq!(cell.loader_fallback(&db), &after);
    assert!(cell.imports_by_sourcing_file(&db).is_empty());
    assert!(cell
        .imports_by_sourcing_file_at(&db, last_offset(&db, cell, "after"))
        .is_empty());
}

#[test]
fn test_notebook_loader_fallback_backdates_resolution_after_position_edit() {
    let mut db = TestDb::new();
    install_packages(&mut db, &["base", "dplyr"]);
    let (_, cells) = open_notebook(&mut db, &["library(dplyr)\nf <- function() missing\n"]);
    let cell = cells[0];
    assert!(cell.resolve(&db, Name::new(&db, "missing")).is_empty());
    let fallback_runs = db.executions("loader_fallback");
    let resolve_runs = db.executions("resolve_(");

    cell.set_source_text_override(&mut db).to(Some(
        "\n\nlibrary(dplyr)\nf <- function() missing\n".to_string(),
    ));
    assert!(cell.resolve(&db, Name::new(&db, "missing")).is_empty());
    assert_eq!(db.executions("loader_fallback"), fallback_runs + 1);
    assert_eq!(db.executions("resolve_("), resolve_runs);
}

#[test]
fn test_notebook_reorder_changes_visibility() {
    let mut db = TestDb::new();
    let (notebook, cells) = open_notebook(&mut db, &["y\n", "y <- 1\n"]);
    let use_offset = last_offset(&db, cells[0], "y");
    assert!(cells[0].resolve_at(&db, use_offset).is_empty());

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

#[test]
fn test_notebook_function_body_sees_later_cell_over_own_cell() {
    // Both cells' top levels write into the notebook environment in document
    // order, so `x <- 2` overwrites `x <- 1` and `f()` sees the overwrite.
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["x <- 1\nf <- function() x\n", "x <- 2\n"]);

    let defs = cells[0].resolve_at(&db, last_offset(&db, cells[0], "x"));

    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[1]);
}

#[test]
fn test_notebook_conditional_successor_keeps_own_binding() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &[
        "x <- 1\nf <- function() x\n",
        "if (flag) x <- 2\n",
    ]);

    assert_notebook_x_resolution(&db, cells[0], &[cells[1], cells[0]]);
}

#[test]
fn test_notebook_multiple_conditional_successors_keep_own_binding() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &[
        "x <- 1\nf <- function() x\n",
        "if (first) x <- 2\n",
        "if (second) x <- 3\n",
    ]);

    assert_notebook_x_resolution(&db, cells[0], &[cells[2], cells[1], cells[0]]);
}

#[test]
fn test_notebook_conditional_successor_stops_at_definite_successor() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &[
        "x <- 1\nf <- function() x\n",
        "x <- 2\n",
        "if (flag) x <- 3\n",
    ]);

    assert_notebook_x_resolution(&db, cells[0], &[cells[2], cells[1]]);
}

#[test]
fn test_notebook_successor_bound_on_both_arms_shadows_own_binding() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &[
        "x <- 1\nf <- function() x\n",
        "if (flag) x <- 2 else x <- 3\n",
    ]);

    assert_notebook_x_resolution(&db, cells[0], &[cells[1], cells[1]]);
}

#[test]
fn test_notebook_conditional_successor_falls_back_without_own_binding() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &[
        "x <- 1\n",
        "f <- function() x\n",
        "if (flag) x <- 2\n",
    ]);

    assert_notebook_x_resolution(&db, cells[1], &[cells[2], cells[0]]);
}

#[test]
fn test_notebook_conditional_predecessor_keeps_earlier_binding() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["x <- 1\n", "if (flag) x <- 2\n", "x\n"]);

    assert_notebook_x_resolution(&db, cells[2], &[cells[1], cells[0]]);
}

#[test]
fn test_notebook_successor_boundness_edit_updates_resolution() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &[
        "x <- 1\nf <- function() x\n",
        "if (flag) x <- 2\n",
    ]);
    assert_notebook_x_resolution(&db, cells[0], &[cells[1], cells[0]]);

    cells[1]
        .set_source_text_override(&mut db)
        .to(Some("x <- 2\n".to_string()));
    assert_notebook_x_resolution(&db, cells[0], &[cells[1]]);

    cells[1]
        .set_source_text_override(&mut db)
        .to(Some("if (flag) x <- 2\n".to_string()));
    assert_notebook_x_resolution(&db, cells[0], &[cells[1], cells[0]]);
}

#[test]
fn test_notebook_conditional_successor_edit_backdates_resolution() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &[
        "x <- 1\nf <- function() x\n",
        "if (flag) x <- 2\n",
    ]);
    assert_notebook_x_resolution(&db, cells[0], &[cells[1], cells[0]]);

    cells[1]
        .set_source_text_override(&mut db)
        .to(Some("\n\nif (flag) x <- 2\n".to_string()));
    assert_notebook_x_resolution(&db, cells[0], &[cells[1], cells[0]]);
    assert_eq!(db.executions("resolve_("), 1);
}

fn assert_notebook_x_resolution(db: &TestDb, cell: File, expected: &[File]) {
    let at_use = cell.resolve_at(db, last_offset(db, cell, "x"));
    let at_eof = cell.resolve(db, Name::new(db, "x"));
    assert_eq!(at_use, at_eof);
    let files: Vec<File> = at_use.iter().map(|def| def.file(db)).collect();
    assert_eq!(files, expected);
}

#[test]
fn test_notebook_function_body_keeps_conditional_local_and_later_cell() {
    // The conditional local takes precedence when `flag` is true. Otherwise,
    // lookup reaches the notebook environment, where the later cell has
    // overwritten this cell's top-level binding.
    let mut db = TestDb::new();
    let own = "x <- 1\nf <- function(flag) {\n  if (flag) x <- 10\n  x\n}\n";
    let (_, cells) = open_notebook(&mut db, &[own, "x <- 2\n"]);

    let defs = cells[0].resolve_at(&db, last_offset(&db, cells[0], "x"));

    assert_eq!(defs.len(), 2);
    assert_eq!(defs[0].file(&db), cells[0]);
    let local_offset = own.find("x <- 10").unwrap();
    assert_eq!(
        usize::from(defs[0].name_range(&db).unwrap().start()),
        local_offset
    );
    assert_eq!(defs[1].file(&db), cells[1]);
}

#[test]
fn test_notebook_nested_closure_keeps_enclosing_function_binding() {
    // `g()` captures `f()`'s local `x`, which shadows the notebook environment
    // even when a later cell writes to it.
    let mut db = TestDb::new();
    let own = "x <- 1\nf <- function() {\n  x <- 10\n  g <- function() x\n}\n";
    let (_, cells) = open_notebook(&mut db, &[own, "x <- 2\n"]);

    let defs = cells[0].resolve_at(&db, last_offset(&db, cells[0], "x"));

    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[0]);
    let local_offset = own.find("x <- 10").unwrap();
    assert_eq!(
        usize::from(defs[0].name_range(&db).unwrap().start()),
        local_offset
    );
}

#[test]
fn test_notebook_top_level_use_keeps_own_cell_over_later_cell() {
    // A top-level use runs while its own cell executes, before the later cell.
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["x <- 1\nx\n", "x <- 2\n"]);

    let defs = cells[0].resolve_at(&db, last_offset(&db, cells[0], "x"));

    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[0]);
}

#[test]
fn test_notebook_function_body_keeps_own_cell_over_earlier_cell() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["x <- 1\n", "x <- 2\nf <- function() x\n"]);

    let defs = cells[1].resolve_at(&db, last_offset(&db, cells[1], "x"));

    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[1]);
}

#[test]
fn test_notebook_conditional_own_cell_falls_back_to_earlier_cell() {
    // `if (cond) x <- 2` may not run, so the earlier cell's binding remains a
    // candidate below it.
    let mut db = TestDb::new();
    let own = "if (cond) x <- 2\nf <- function() x\n";
    let (_, cells) = open_notebook(&mut db, &["x <- 1\n", own]);

    let defs = cells[1].resolve_at(&db, last_offset(&db, cells[1], "x"));

    assert_eq!(defs.len(), 2);
    assert_eq!(defs[0].file(&db), cells[1]);
    assert_eq!(defs[1].file(&db), cells[0]);
}

#[test]
fn test_notebook_reorder_updates_deferred_shadowing() {
    let mut db = TestDb::new();
    let (notebook, cells) = open_notebook(&mut db, &["x <- 0\nf <- function() x\n", "x <- 1\n"]);

    let defs = cells[0].resolve_at(&db, last_offset(&db, cells[0], "x"));
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[1]);

    // Moving this cell changes precedence without changing its sibling list.
    // Its position in the cached environment sequence must change too.
    notebook.set_cells(&mut db).to(vec![cells[1], cells[0]]);
    let defs = cells[0].resolve_at(&db, last_offset(&db, cells[0], "x"));
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[0]);
}

#[test]
fn test_notebook_reorder_updates_tracked_resolve() {
    let mut db = TestDb::new();
    let (notebook, cells) = open_notebook(&mut db, &["x <- 0\n", "x <- 1\n"]);

    let defs = cells[0].resolve(&db, Name::new(&db, "x"));
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[1]);

    // The sibling list is unchanged. The cell's changed position in the
    // environment sequence must invalidate `resolve()` so its own binding wins.
    notebook.set_cells(&mut db).to(vec![cells[1], cells[0]]);
    let defs = cells[0].resolve(&db, Name::new(&db, "x"));
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[0]);
}

#[test]
fn test_notebook_successor_edit_backdates_tracked_resolve() {
    // A position-only edit preserves `Definition` identity, so `resolve()`
    // should stay cached even though the successor's source text changes.
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["f <- function() x\n", "x <- 1\n"]);

    let defs = cells[0].resolve(&db, Name::new(&db, "x"));
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[1]);

    cells[1]
        .set_source_text_override(&mut db)
        .to(Some("\n\nx <- 1\n".to_string()));

    let defs = cells[0].resolve(&db, Name::new(&db, "x"));
    assert_eq!(defs.len(), 1);
    assert_eq!(defs[0].file(&db), cells[1]);
    // Match `resolve_(` to exclude executions of `resolve_export()`.
    assert_eq!(db.executions("resolve_("), 1);
}

#[test]
fn test_notebook_text_edit_does_not_rebuild_cell_index() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["old <- 1\n", "old\n"]);

    let _ = cells[1].imports(&db);
    let index_runs = db.executions("notebook_cell_index");

    cells[0]
        .set_source_text_override(&mut db)
        .to(Some("new <- 1\n".to_string()));
    let _ = cells[1].imports(&db);

    assert_eq!(db.executions("notebook_cell_index"), index_runs);
}

#[test]
fn test_unrelated_notebook_change_backdates_cell_layers() {
    let mut db = TestDb::new();
    let (_, cells) = open_notebook(&mut db, &["a <- 1\n"]);
    let script = File::new(
        &db,
        file_path("script.R"),
        FileRevision::zero(),
        Some("s <- 1\n".to_string()),
        None,
    );

    let _ = cells[0].imports(&db);
    let _ = script.imports(&db);
    let layers_runs = db.executions("cross_file_layers");

    // Neither file's layers re-execute because `notebook_by_cell()` returns
    // unchanged membership, even though opening a notebook rebuilds the index.
    let (other, _) = add_notebook(&mut db, "other", &["b <- 1\n"]);
    let _ = cells[0].imports(&db);
    let _ = script.imports(&db);
    assert_eq!(db.executions("cross_file_layers"), layers_runs);

    // Closing the cell's notebook invalidates its layers through changed
    // membership. The script's membership remains `None`, so its layers stay cached.
    db.open_notebooks().set_notebooks(&mut db).to(vec![other]);
    let _ = cells[0].imports(&db);
    let _ = script.imports(&db);
    assert_eq!(db.executions("cross_file_layers"), layers_runs + 1);
}

#[test]
fn test_notebook_reorder_reexecutes_cell_layers() {
    let mut db = TestDb::new();
    let (notebook, cells) = open_notebook(&mut db, &["y\n", "y <- 1\n"]);

    let _ = cells[0].imports(&db);
    let layers_runs = db.executions("cross_file_layers");

    // Cell order must still invalidate the layers when membership is unchanged.
    // The load context depends directly on `Notebook::cells()`.
    notebook.set_cells(&mut db).to(vec![cells[1], cells[0]]);
    let _ = cells[0].imports(&db);

    assert_eq!(db.executions("cross_file_layers"), layers_runs + 1);
}
