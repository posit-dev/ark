//! Unit tests for [`crate::stale`] eviction routing, written against
//! the public API ([`RootExt::set_stale`]) rather than the internal
//! free function so they double as call-pattern documentation.

use std::collections::HashSet;

use aether_url::UrlId;
use oak_db::DbInputs;
use oak_db::File;
use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use oak_package_metadata::namespace::Namespace;
use salsa::Setter;
use url::Url;

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
