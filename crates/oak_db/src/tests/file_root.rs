use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::intern_file;
use crate::intern_package;
use crate::tests::test_db::file_url;
use crate::tests::test_db::library_root;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::Db;

#[test]
fn root_returns_none_for_orphan_file_outside_workspace() {
    let mut db = TestDb::new();
    let file = intern_file(&mut db, file_url("orphan.R"), String::new(), None);

    assert_eq!(file.root(&db), None);
}

#[test]
fn root_finds_containing_workspace_for_orphan_file() {
    let mut db = TestDb::new();
    let workspace = workspace_root(&db, "proj");
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    let file = intern_file(&mut db, file_url("proj/scripts/foo.R"), String::new(), None);
    assert_eq!(file.root(&db), Some(workspace));
}

#[test]
fn root_returns_longest_prefix_for_orphan_file() {
    let mut db = TestDb::new();
    let outer = workspace_root(&db, "proj");
    let inner = workspace_root(&db, "proj/inner");
    db.workspace_roots()
        .set_roots(&mut db)
        .to(vec![outer, inner]);

    let inner_file = intern_file(&mut db, file_url("proj/inner/foo.R"), String::new(), None);
    assert_eq!(inner_file.root(&db), Some(inner));

    let outer_file = intern_file(&mut db, file_url("proj/foo.R"), String::new(), None);
    assert_eq!(outer_file.root(&db), Some(outer));
}

#[test]
fn root_dispatches_through_package_when_set() {
    let mut db = TestDb::new();
    let pkg_root = library_root(&db, "libs/mypkg");
    let pkg = intern_package(
        &mut db,
        pkg_root,
        "mypkg".to_string(),
        Some("1.0.0".to_string()),
        Namespace::default(),
        Vec::new(),
        None,
    );

    // The file is not interned under any workspace prefix; without the
    // package back-pointer `root()` would return `None`. With it, the
    // file dispatches through `Package.root`.
    let file = intern_file(
        &mut db,
        file_url("libs/mypkg/R/foo.R"),
        String::new(),
        Some(pkg),
    );
    assert_eq!(file.root(&db), Some(pkg_root));
}
