//! Unit tests for [`crate::stale`] eviction routing, written against
//! the public API ([`RootExt::set_stale`]) rather than the internal
//! free function so they double as call-pattern documentation.

use std::collections::HashSet;

use aether_url::UrlId;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;
use url::Url;

use crate::inputs::DbExt;
use crate::inputs::RootExt;

fn file_url(s: &str) -> UrlId {
    UrlId::from_canonical(Url::parse(&format!("file://{s}")).unwrap())
}

#[test]
fn test_set_stale_routes_editor_owned_to_orphan() {
    let mut db = OakDatabase::new();
    let root = Root::new(&db, file_url("/proj"), RootKind::Workspace, vec![], vec![]);
    let file = File::new(&db, file_url("/proj/foo.R"), "x".to_string(), None);
    root.set_scripts(&mut db).to(vec![file]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let mut owned = HashSet::new();
    owned.insert(file_url("/proj/foo.R"));
    root.set_stale(&mut db, Some(&owned));

    assert!(db.orphan_root().files(&db).contains(&file));
    assert!(!db.stale_root().files(&db).contains(&file));
    assert_eq!(file.package(&db), None);
}

#[test]
fn test_set_stale_routes_non_editor_owned_to_stale() {
    let mut db = OakDatabase::new();
    let root = Root::new(&db, file_url("/proj"), RootKind::Workspace, vec![], vec![]);
    let file = File::new(&db, file_url("/proj/foo.R"), "x".to_string(), None);
    root.set_scripts(&mut db).to(vec![file]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    root.set_stale(&mut db, None);

    assert!(!db.orphan_root().files(&db).contains(&file));
    assert!(db.stale_root().files(&db).contains(&file));
}

#[test]
fn test_set_stale_clears_package_on_editor_owned_package_file() {
    // The doc claim being tested: an evicted editor-owned package
    // file loses its package association when it lands in orphan.
    // The package itself goes to stale.
    let mut db = OakDatabase::new();
    let root = Root::new(&db, file_url("/proj"), RootKind::Workspace, vec![], vec![]);
    let pkg = Package::new(
        &db,
        file_url("/proj/DESCRIPTION"),
        "p".to_string(),
        None,
        Namespace::default(),
        vec![],
        Vec::new(),
        None,
    );
    let file = File::new(&db, file_url("/proj/R/a.R"), "x".to_string(), Some(pkg));
    pkg.set_files(&mut db).to(vec![file]);
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let mut owned = HashSet::new();
    owned.insert(file_url("/proj/R/a.R"));
    root.set_stale(&mut db, Some(&owned));

    assert!(db.orphan_root().files(&db).contains(&file));
    assert_eq!(file.package(&db), None);
    assert!(db.stale_root().packages(&db).contains(&pkg));
}

#[test]
fn test_set_stale_routes_pkg_scripts_to_stale() {
    // A non-editor-owned file in `pkg.scripts` (e.g. tests/test-foo.R)
    // should go to stale on root eviction, alongside the package itself.
    let mut db = OakDatabase::new();
    let root = Root::new(&db, file_url("/proj"), RootKind::Workspace, vec![], vec![]);
    let pkg = Package::new(
        &db,
        file_url("/proj/DESCRIPTION"),
        "p".to_string(),
        None,
        Namespace::default(),
        vec![],
        Vec::new(),
        None,
    );
    let test_file = File::new(
        &db,
        file_url("/proj/tests/test-foo.R"),
        "t\n".to_string(),
        Some(pkg),
    );
    pkg.set_scripts(&mut db).to(vec![test_file]);
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    root.set_stale(&mut db, None);

    assert!(db.stale_root().files(&db).contains(&test_file));
    assert!(!db.orphan_root().files(&db).contains(&test_file));
    assert!(db.stale_root().packages(&db).contains(&pkg));
}

#[test]
fn test_set_stale_routes_editor_owned_pkg_scripts_to_orphan() {
    // An open `pkg/tests/test-foo.R` buffer should survive root eviction
    // in orphan, with its package backpointer cleared so analysis treats
    // it as a standalone script while the workspace is gone.
    let mut db = OakDatabase::new();
    let root = Root::new(&db, file_url("/proj"), RootKind::Workspace, vec![], vec![]);
    let pkg = Package::new(
        &db,
        file_url("/proj/DESCRIPTION"),
        "p".to_string(),
        None,
        Namespace::default(),
        vec![],
        Vec::new(),
        None,
    );
    let test_file = File::new(
        &db,
        file_url("/proj/tests/test-foo.R"),
        "t\n".to_string(),
        Some(pkg),
    );
    pkg.set_scripts(&mut db).to(vec![test_file]);
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    let mut owned = HashSet::new();
    owned.insert(file_url("/proj/tests/test-foo.R"));
    root.set_stale(&mut db, Some(&owned));

    assert!(db.orphan_root().files(&db).contains(&test_file));
    assert_eq!(test_file.package(&db), None);
    assert!(db.stale_root().packages(&db).contains(&pkg));
}

#[test]
fn test_close_editor_moves_orphan_file_to_stale() {
    // An untitled / out-of-workspace buffer lives in orphan. On close,
    // it should move to stale so the entity survives for a possible
    // re-open instead of leaking as a zombie in orphan.
    let mut db = OakDatabase::new();
    let url = file_url("/scratch/foo.R");
    let file = db.upsert_editor(url.clone(), "hello\n".to_string());
    assert!(db.orphan_root().files(&db).contains(&file));

    db.close_editor(&url);

    assert!(!db.orphan_root().files(&db).contains(&file));
    assert!(db.stale_root().files(&db).contains(&file));
}

#[test]
fn test_upsert_editor_resurrects_from_stale() {
    // Closing then reopening the same URL reuses the same `File` entity.
    use salsa::plumbing::AsId;

    let mut db = OakDatabase::new();
    let url = file_url("/scratch/foo.R");
    let id_before = db.upsert_editor(url.clone(), "v1\n".to_string()).as_id();
    db.close_editor(&url);

    let id_after = db.upsert_editor(url.clone(), "v2\n".to_string()).as_id();
    assert_eq!(id_before, id_after);

    // The file is back in orphan, content from the second open.
    let file = db.file_by_url(&url).unwrap();
    assert!(db.orphan_root().files(&db).contains(&file));
    assert!(!db.stale_root().files(&db).contains(&file));
    assert_eq!(file.contents(&db), "v2\n");
}

#[test]
fn test_close_editor_is_noop_for_file_in_live_root() {
    // The editor's release doesn't disturb the scanner's classification.
    // A file inside a live root's `packages` / `scripts` stays put.
    let mut db = OakDatabase::new();
    let root = Root::new(&db, file_url("/proj"), RootKind::Workspace, vec![], vec![]);
    let url = file_url("/proj/foo.R");
    let file = File::new(&db, url.clone(), "x".to_string(), None);
    root.set_scripts(&mut db).to(vec![file]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    db.close_editor(&url);

    let root = db.workspace_roots().roots(&db)[0];
    assert!(root.scripts(&db).contains(&file));
    assert!(!db.orphan_root().files(&db).contains(&file));
    assert!(!db.stale_root().files(&db).contains(&file));
}
