use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::tests::test_db::file_url;
use crate::tests::test_db::library_root;
use crate::tests::test_db::workspace_root;
use crate::DbInputs;
use crate::File;
use crate::OakDatabase;
use crate::Package;

#[test]
fn test_root_returns_none_for_orphan_file_outside_workspace() {
    let db = OakDatabase::new();
    let file = File::new(&db, file_url("orphan.R"), String::new());

    assert_eq!(file.root(&db), None);
}

#[test]
fn test_root_finds_containing_workspace_for_orphan_file() {
    let mut db = OakDatabase::new();
    let workspace = workspace_root(&db, "proj");
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);

    let file = File::new(&db, file_url("proj/scripts/foo.R"), String::new());
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

    let inner_file = File::new(&db, file_url("proj/inner/foo.R"), String::new());
    assert_eq!(inner_file.root(&db), Some(inner));

    let outer_file = File::new(&db, file_url("proj/foo.R"), String::new());
    assert_eq!(outer_file.root(&db), Some(outer));
}

#[test]
fn test_root_dispatches_through_library_package_when_set() {
    let mut db = OakDatabase::new();
    let pkg_root = library_root(&db, "libs/mypkg");
    let pkg = Package::new(
        &db,
        file_url("libs/mypkg/DESCRIPTION"),
        "mypkg".to_string(),
        Some("1.0.0".to_string()),
        Namespace::default(),
        Vec::new(),
        None,
    );
    // File placed in `pkg.files`. `root()` derives the package from that
    // containment and dispatches through `Db::root_by_package` rather than
    // falling back to the URL-prefix walk against workspace roots.
    let file = File::new(&db, file_url("libs/mypkg/R/foo.R"), String::new());
    pkg.set_files(&mut db).to(vec![file]);
    pkg_root.set_packages(&mut db).to(vec![pkg]);
    db.library_roots().set_roots(&mut db).to(vec![pkg_root]);

    assert_eq!(file.root(&db), Some(pkg_root));
}

#[test]
fn test_root_dispatches_through_workspace_package_when_set() {
    // Same dispatch as the library case, but the owning root is a
    // `Workspace` kind. The URL-prefix fallback is *not* consulted here
    // because the file belongs to a package.
    let mut db = OakDatabase::new();
    let pkg_root = workspace_root(&db, "proj");
    let pkg = Package::new(
        &db,
        file_url("proj/DESCRIPTION"),
        "mypkg".to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        None,
    );
    let file = File::new(&db, file_url("proj/R/foo.R"), String::new());
    pkg.set_files(&mut db).to(vec![file]);
    pkg_root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![pkg_root]);

    assert_eq!(file.root(&db), Some(pkg_root));
}
