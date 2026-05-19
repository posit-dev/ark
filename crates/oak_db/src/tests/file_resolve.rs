use salsa::Setter;

use crate::tests::test_db::file_url;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::Name;

/// Build a workspace root at `w` populated with the given files.
/// Returns the file handles in the same order. Registers the root with
/// `WorkspaceRoots` so `file_by_url` can find the files for cross-file
/// resolution.
fn setup_workspace(db: &mut TestDb, scripts: &[(&str, &str)]) -> Vec<File> {
    let root = workspace_root(db, "w");
    let files: Vec<File> = scripts
        .iter()
        .map(|(name, contents)| File::new(db, file_url(name), contents.to_string(), None))
        .collect();
    root.set_scripts(db).to(files.clone());
    db.workspace_roots().set_roots(db).to(vec![root]);
    files
}

fn name<'db>(db: &'db TestDb, text: &str) -> Name<'db> {
    Name::new(db, text)
}

#[test]
fn test_resolve_local_name_lands_on_owning_file() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n")]);
    let file = files[0];

    let resolution = file.resolve(&db, name(&db, "x")).expect("x should resolve");
    assert_eq!(resolution.file, file);
    assert_eq!(resolution.name.text(&db).as_str(), "x");
    // `semantic_index.file_exports()` records the binding's *name*
    // range, just the `x` identifier in `x <- 1`.
    assert_eq!(usize::from(resolution.range.start()), 0);
    assert_eq!(usize::from(resolution.range.end()), 1);
}

#[test]
fn test_unresolved_name_returns_none() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n")]);
    let file = files[0];
    assert!(file.resolve(&db, name(&db, "nope")).is_none());
}

#[test]
fn test_resolve_chases_source_forwarding_to_origin_file() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/helpers.R", "helper <- function() 1\n"),
        ("w/analysis.R", "source(\"helpers.R\")\n"),
    ]);
    let helpers = files[0];
    let analysis = files[1];

    let resolution = analysis
        .resolve(&db, name(&db, "helper"))
        .expect("helper should resolve through source()");

    assert_eq!(resolution.file, helpers);
    assert_eq!(resolution.name.text(&db).as_str(), "helper");
}

#[test]
fn test_resolve_chases_two_step_source_chain() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/leaf.R", "deep <- 1\n"),
        ("w/mid.R", "source(\"leaf.R\")\n"),
        ("w/top.R", "source(\"mid.R\")\n"),
    ]);
    let leaf = files[0];
    let top = files[2];

    let resolution = top
        .resolve(&db, name(&db, "deep"))
        .expect("deep should chase through mid -> leaf");

    assert_eq!(resolution.file, leaf);
    assert_eq!(resolution.name.text(&db).as_str(), "deep");
}

#[test]
fn test_resolve_is_cached_across_repeat_calls() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n")]);
    let file = files[0];

    let _ = file.resolve(&db, name(&db, "x"));
    let _ = file.resolve(&db, name(&db, "x"));

    // Tracked: the second call hits the salsa cache.
    assert_eq!(db.executions("resolve"), 1);
}

#[test]
fn test_resolve_in_cyclic_source_returns_none_without_panicking() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "source(\"b.R\")\na_local <- 1\n"),
        ("w/b.R", "source(\"a.R\")\nb_local <- 2\n"),
    ]);
    let a = files[0];
    let b = files[1];

    // a.R sources b.R; b.R sources a.R. Both sides' `exports` cycle
    // to empty via `cycle_result`, so `resolve` returns `None` for
    // names that would otherwise be found in those exports. The
    // point of the test is that resolution terminates cleanly rather
    // than panicking.
    assert!(a.resolve(&db, name(&db, "a_local")).is_none());
    assert!(b.resolve(&db, name(&db, "b_local")).is_none());
}
