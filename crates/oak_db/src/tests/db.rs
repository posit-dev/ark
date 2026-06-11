use std::collections::HashSet;

use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::tests::test_db::file_url;
use crate::tests::test_db::library_root;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::Db;
use crate::DbInputs;
use crate::File;
use crate::Package;

#[test]
fn test_file_by_path_finds_workspace_script() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "proj");
    let file = File::new(&db, file_url("proj/a.R"), String::new(), None);
    root.set_scripts(&mut db).to(vec![file]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(db.file_by_path(&file_url("proj/a.R")), Some(file));
}

#[test]
fn test_file_by_path_finds_workspace_package_file() {
    let mut db = TestDb::new();
    let root = workspace_root(&db, "proj");
    // Construct the back-pointer + forward-edge pair: create the
    // `Package` with an empty `files` list, then the `File` with the
    // `package` set, then attach the file via `set_files`. Matches the
    // shape `oak_scan`'s placement-preserving helpers will produce.
    let pkg = Package::new(
        &db,
        file_url("proj/DESCRIPTION"),
        "mypkg".to_string(),
        None,
        Namespace::default(),
        vec![],
        Vec::new(),
        None,
    );
    let file = File::new(&db, file_url("proj/R/foo.R"), String::new(), Some(pkg));
    pkg.set_files(&mut db).to(vec![file]);
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(db.file_by_path(&file_url("proj/R/foo.R")), Some(file));
}

#[test]
fn test_file_by_path_finds_library_package_file() {
    let mut db = TestDb::new();
    let lib = library_root(&db, "libs");
    let pkg = Package::new(
        &db,
        file_url("libs/foo/DESCRIPTION"),
        "foo".to_string(),
        Some("1.0.0".to_string()),
        Namespace::default(),
        vec![],
        Vec::new(),
        None,
    );
    let file = File::new(&db, file_url("libs/foo/R/a.R"), String::new(), Some(pkg));
    pkg.set_files(&mut db).to(vec![file]);
    lib.set_packages(&mut db).to(vec![pkg]);
    db.library_roots().set_roots(&mut db).to(vec![lib]);

    assert_eq!(db.file_by_path(&file_url("libs/foo/R/a.R")), Some(file));
}

#[test]
fn test_file_by_path_finds_orphan_file() {
    let mut db = TestDb::new();
    let file = File::new(&db, file_url("untitled.R"), String::new(), None);
    db.orphan_root()
        .set_files(&mut db)
        .to(HashSet::from([file]));

    assert_eq!(db.file_by_path(&file_url("untitled.R")), Some(file));
}

#[test]
fn test_file_by_path_returns_none_when_absent() {
    let db = TestDb::new();
    assert_eq!(db.file_by_path(&file_url("missing.R")), None);
}

#[test]
fn test_root_path_index_invalidates_per_root() {
    // Headline claim of the per-root scaffold: writing to one root's
    // contents doesn't invalidate another root's index. We assert it
    // by counting salsa `WillExecute` events on `root_path_index`.
    let mut db = TestDb::new();
    let root_a = workspace_root(&db, "a");
    let root_b = workspace_root(&db, "b");
    let file_a = File::new(&db, file_url("a/file.R"), String::new(), None);
    let file_b = File::new(&db, file_url("b/file.R"), String::new(), None);
    root_a.set_scripts(&mut db).to(vec![file_a]);
    root_b.set_scripts(&mut db).to(vec![file_b]);
    db.workspace_roots()
        .set_roots(&mut db)
        .to(vec![root_a, root_b]);

    // First lookup of `file_a` hits root A's index only (the walk
    // returns early). Second lookup of `file_b` finds nothing in A's
    // cached index, then executes B's. Total: 2.
    let _ = db.file_by_path(&file_url("a/file.R"));
    let _ = db.file_by_path(&file_url("b/file.R"));
    assert_eq!(db.executions("root_path_index"), 2);

    // Add a script to root B. B's index invalidates; A's stays cached.
    let file_b2 = File::new(&db, file_url("b/other.R"), String::new(), None);
    root_b.set_scripts(&mut db).to(vec![file_b, file_b2]);

    // Look up the file in A. A's index is still cached, no re-exec.
    let _ = db.file_by_path(&file_url("a/file.R"));
    assert_eq!(db.executions("root_path_index"), 2);

    // Look up the file in B. A still cached; B's index re-executes.
    let _ = db.file_by_path(&file_url("b/file.R"));
    assert_eq!(db.executions("root_path_index"), 3);
}
