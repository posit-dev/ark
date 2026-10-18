//! Eviction routing and open-buffer visibility after [`RootExt::evict()`]
//! and [`keep_open_files_visible()`], as used by public entry points.

use aether_path::FilePath;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::FileRevision;
use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::PackageFiles;
use oak_db::Root;
use oak_db::RootKind;
use oak_db::SourceDb;
use salsa::Setter;
use url::Url;

use crate::inputs::keep_open_files_visible;
use crate::inputs::RootExt;

fn file_path(s: &str) -> FilePath {
    FilePath::from_url(&Url::parse(&format!("file://{s}")).unwrap())
}

#[test]
fn test_evict_routes_editor_owned_to_orphan() {
    let mut db = OakDatabase::new();
    let root = Root::new(
        &db,
        file_path("/proj"),
        RootKind::Workspace,
        vec![],
        vec![],
        vec![],
    );
    let file = File::new(
        &db,
        file_path("/proj/foo.R"),
        oak_db::FileRevision::zero(),
        Some("x".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![file]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    db.set_editor_buffer(file, "x".to_string());
    root.evict(&mut db);
    keep_open_files_visible(&mut db);

    assert!(db.orphan_root().files(&db).contains(&file));
    assert!(root.scripts(&db).is_empty());
    assert_eq!(file.package(&db), None);
}

#[test]
fn test_evict_unpublishes_non_editor_owned() {
    let mut db = OakDatabase::new();
    let root = Root::new(
        &db,
        file_path("/proj"),
        RootKind::Workspace,
        vec![],
        vec![],
        vec![],
    );
    let file = File::new(
        &db,
        file_path("/proj/foo.R"),
        oak_db::FileRevision::zero(),
        Some("x".to_string()),
        None,
    );
    root.set_scripts(&mut db).to(vec![file]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    root.evict(&mut db);
    db.workspace_roots().set_roots(&mut db).to(vec![]);
    keep_open_files_visible(&mut db);

    assert!(!db.orphan_root().files(&db).contains(&file));
    assert!(root.scripts(&db).is_empty());
    assert_eq!(db.file_by_path(&file_path("/proj/foo.R")), None);
}

#[test]
fn test_evict_clears_package_on_editor_owned_package_file() {
    let mut db = OakDatabase::new();
    let root = Root::new(
        &db,
        file_path("/proj"),
        RootKind::Workspace,
        vec![],
        vec![],
        vec![],
    );
    let pkg = Package::new(
        &db,
        file_path("/proj/DESCRIPTION"),
        "p".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        PackageFiles::default(),
        Vec::new(),
    );
    let file = File::new(
        &db,
        file_path("/proj/R/a.R"),
        oak_db::FileRevision::zero(),
        Some("x".to_string()),
        Some(pkg),
    );
    pkg.set_source_files(&mut db, vec![file]);
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    db.set_editor_buffer(file, "x".to_string());
    root.evict(&mut db);
    keep_open_files_visible(&mut db);

    assert!(db.orphan_root().files(&db).contains(&file));
    assert_eq!(file.package(&db), None);
    assert!(pkg.files(&db).is_empty());
    assert!(root.packages(&db).is_empty());
}

#[test]
fn test_evict_unpublishes_pkg_scripts() {
    let mut db = OakDatabase::new();
    let root = Root::new(
        &db,
        file_path("/proj"),
        RootKind::Workspace,
        vec![],
        vec![],
        vec![],
    );
    let pkg = Package::new(
        &db,
        file_path("/proj/DESCRIPTION"),
        "p".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        PackageFiles::default(),
        Vec::new(),
    );
    let test_file = File::new(
        &db,
        file_path("/proj/tests/test-foo.R"),
        oak_db::FileRevision::zero(),
        Some("t\n".to_string()),
        Some(pkg),
    );
    pkg.set_scripts(&mut db).to(vec![test_file]);
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    root.evict(&mut db);

    assert!(!db.orphan_root().files(&db).contains(&test_file));
    assert!(pkg.scripts(&db).is_empty());
    assert_eq!(test_file.package(&db), None);
    assert_eq!(db.file_by_path(&file_path("/proj/tests/test-foo.R")), None);
}

#[test]
fn test_evict_routes_editor_owned_pkg_scripts_to_orphan() {
    // An open package test file needs orphan membership and no package
    // association so analysis treats it as a standalone script after eviction.
    let mut db = OakDatabase::new();
    let root = Root::new(
        &db,
        file_path("/proj"),
        RootKind::Workspace,
        vec![],
        vec![],
        vec![],
    );
    let pkg = Package::new(
        &db,
        file_path("/proj/DESCRIPTION"),
        "p".to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        PackageFiles::default(),
        Vec::new(),
    );
    let test_file = File::new(
        &db,
        file_path("/proj/tests/test-foo.R"),
        oak_db::FileRevision::zero(),
        Some("t\n".to_string()),
        Some(pkg),
    );
    pkg.set_scripts(&mut db).to(vec![test_file]);
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    db.set_editor_buffer(test_file, "t\n".to_string());
    root.evict(&mut db);
    keep_open_files_visible(&mut db);

    assert!(db.orphan_root().files(&db).contains(&test_file));
    assert_eq!(test_file.package(&db), None);
    assert!(pkg.scripts(&db).is_empty());
}
