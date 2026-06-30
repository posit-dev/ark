use salsa::Setter;

use crate::tests::test_db::file_path;
use crate::tests::test_db::library_root;
use crate::tests::test_db::workspace_root;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::OakDatabase;
use crate::Package;

#[test]
fn test_root_returns_none_for_orphan_file_outside_workspace() {
    let db = OakDatabase::new();
    let file = File::new(&db, file_path("orphan.R"), FileRevision::zero(), None, None);

    assert_eq!(file.root(&db), None);
}

#[test]
fn test_root_finds_containing_workspace_for_orphan_file() {
    let mut db = OakDatabase::new();
    let workspace = workspace_root(&db, "proj");
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    let file = File::new(
        &db,
        file_path("proj/scripts/foo.R"),
        FileRevision::zero(),
        None,
        None,
    );
    assert_eq!(file.root(&db), Some(workspace));
}

#[test]
fn test_root_returns_longest_prefix_for_orphan_file() {
    let mut db = OakDatabase::new();
    let outer = workspace_root(&db, "proj");
    let inner = workspace_root(&db, "proj/inner");
    db.workspace_roots()
        .set_roots(&mut db)
        .to(vec![outer, inner]);

    let inner_file = File::new(
        &db,
        file_path("proj/inner/foo.R"),
        FileRevision::zero(),
        None,
        None,
    );
    assert_eq!(inner_file.root(&db), Some(inner));

    let outer_file = File::new(
        &db,
        file_path("proj/foo.R"),
        FileRevision::zero(),
        None,
        None,
    );
    assert_eq!(outer_file.root(&db), Some(outer));
}

#[test]
fn test_root_dispatches_through_library_package_when_set() {
    let mut db = OakDatabase::new();
    let pkg_root = library_root(&db, "libs/mypkg");
    let pkg = Package::new(
        &db,
        file_path("libs/mypkg/DESCRIPTION"),
        "mypkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        Vec::new(),
        Vec::new(),
    );
    pkg_root.set_packages(&mut db).to(vec![pkg]);
    db.library_roots().set_roots(&mut db).to(vec![pkg_root]);

    // File created with package back-pointer set. `root()` dispatches
    // through `Db::root_by_package` rather than falling back to the URL-
    // prefix walk against workspace roots.
    let file = File::new(
        &db,
        file_path("libs/mypkg/R/foo.R"),
        FileRevision::zero(),
        None,
        Some(pkg),
    );
    assert_eq!(file.root(&db), Some(pkg_root));
}

#[test]
fn test_root_dispatches_through_workspace_package_when_set() {
    // Same dispatch as the library case, but the owning root is a
    // `Workspace` kind. The URL-prefix fallback is *not* consulted here
    // because `package` is set.
    let mut db = OakDatabase::new();
    let pkg_root = workspace_root(&db, "proj");
    let pkg = Package::new(
        &db,
        file_path("proj/DESCRIPTION"),
        "mypkg".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        Vec::new(),
        Vec::new(),
    );
    pkg_root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![pkg_root]);

    let file = File::new(
        &db,
        file_path("proj/R/foo.R"),
        FileRevision::zero(),
        None,
        Some(pkg),
    );
    assert_eq!(file.root(&db), Some(pkg_root));
}
