//! Tests the distinctions preserved by resolution observations.
//!
//! Losing a distinction would make the incremental and fresh results compare
//! equal while they describe different bindings.

use salsa::Setter;

use crate::fuzz::oracle::observe::observe;
use crate::fuzz::oracle::observe::KindTag;
use crate::fuzz::oracle::observe::Observation;
use crate::test_path::file_path;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::Name;

fn setup_workspace(db: &mut TestDb, scripts: &[(&str, &str)]) -> Vec<File> {
    let root = workspace_root(db, "w");
    let files: Vec<File> = scripts
        .iter()
        .map(|(name, contents)| {
            File::new(
                db,
                file_path(name),
                FileRevision::zero(),
                Some(contents.to_string()),
                None,
            )
        })
        .collect();
    root.set_scripts(db).to(files.clone());
    db.workspace_roots().set_roots(db).to(vec![root]);
    files
}

fn observation(db: &TestDb, file: File, text: &str) -> Observation {
    let definitions = file.resolve(db, Name::new(db, text));
    observe(db, &definitions)
}

#[test]
fn test_projects_a_local_binding() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n")]);

    let observed = observation(&db, files[0], "x");

    assert_eq!(observed.resolved.len(), 1);
    assert_eq!(observed.resolved[0].path, "w/a.R");
    assert_eq!(observed.resolved[0].name, "x");
    assert_eq!(observed.resolved[0].kind, KindTag::Assignment);
    assert_eq!(observed.resolved[0].forward, None);
}

#[test]
fn test_separates_two_names() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\ny <- 2\n")]);

    let first = observation(&db, files[0], "x");
    let second = observation(&db, files[0], "y");

    assert_ne!(first, second);
}

#[test]
fn test_separates_the_same_name_in_two_files() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n"), ("w/b.R", "x <- 1\n")]);

    let first = observation(&db, files[0], "x");
    let second = observation(&db, files[1], "x");

    assert_ne!(first, second);
    assert_eq!(first.resolved[0].path, "w/a.R");
    assert_eq!(second.resolved[0].path, "w/b.R");
}

#[test]
fn test_separates_two_binding_kinds() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[
        ("w/a.R", "x <- 1\n"),
        ("w/b.R", "for (x in 1) NULL\n"),
    ]);

    let assignment = observation(&db, files[0], "x");
    let for_variable = observation(&db, files[1], "x");

    assert_eq!(assignment.resolved[0].kind, KindTag::Assignment);
    assert_eq!(for_variable.resolved[0].kind, KindTag::ForVariable);
}

/// Only the ranges differ, so a projection without them would report these bindings as equal.
#[test]
fn test_separates_a_shifted_range() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n"), ("w/b.R", "\n\nx <- 1\n")]);

    let first = observation(&db, files[0], "x");
    let second = observation(&db, files[1], "x");

    assert_ne!(first.resolved[0].range, second.resolved[0].range);
}

/// Both `if` arms bind `x` at file scope. The same path, name, and kind require
/// ranges to keep the definitions distinct.
#[test]
fn test_keeps_two_bindings_of_one_name_apart() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "if (cond) x <- 1 else x <- 2\n")]);

    let observed = observation(&db, files[0], "x");

    assert_eq!(observed.resolved.len(), 2);
    assert_eq!(observed.resolved[0].path, observed.resolved[1].path);
    assert_eq!(observed.resolved[0].name, observed.resolved[1].name);
    assert_ne!(observed.resolved[0].range, observed.resolved[1].range);
    assert!(observed.resolved[0].range.start() < observed.resolved[1].range.start());
}

/// Verify that observation performs no query while a cold query remains
/// available. Execution counts cannot distinguish a memo hit from a call that
/// never ran.
#[test]
fn test_projection_executes_no_query() {
    let mut db = TestDb::new();
    let files = setup_workspace(&mut db, &[("w/a.R", "x <- 1\n")]);

    let definitions = files[0].resolve(&db, Name::new(&db, "x"));
    let executions = db.executions("");

    let _ = observe(&db, &definitions);

    assert_eq!(db.executions(""), executions);

    // Execute a query resolution left cold to verify that the counter
    // increments.
    let _ = files[0].diagnostics(&db);
    assert!(db.executions("") > executions);
}
