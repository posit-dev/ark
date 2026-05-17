use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::tests::test_db::file_url;
use crate::tests::test_db::library_root;
use crate::tests::test_db::workspace_root;
use crate::Db;
use crate::File;
use crate::OakDatabase;
use crate::Package;
use crate::Root;

fn make_workspace_package(db: &mut OakDatabase, name: &str) -> (Root, Package) {
    let root = workspace_root(db, &format!("workspace/{name}"));
    let pkg = db.set_package(
        root,
        name.to_string(),
        None,
        Namespace::default(),
        Vec::new(),
        None,
    );
    root.set_packages(db).to(vec![pkg]);
    (root, pkg)
}

fn make_installed_package(db: &mut OakDatabase, name: &str) -> (Root, Package) {
    let root = library_root(db, &format!("libs/{name}"));
    let pkg = db.set_package(
        root,
        name.to_string(),
        Some("1.0.0".to_string()),
        Namespace::default(),
        Vec::new(),
        None,
    );
    root.set_packages(db).to(vec![pkg]);
    (root, pkg)
}

fn make_script(db: &mut OakDatabase, name: &str) -> File {
    db.set_file(file_url(name), String::new(), None)
}

#[test]
fn package_by_name_finds_workspace_package() {
    let mut db = OakDatabase::new();
    let (root, pkg) = make_workspace_package(&mut db, "rlang");
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(db.package_by_name("rlang"), Some(pkg));
}

#[test]
fn package_by_name_falls_back_to_installed() {
    let mut db = OakDatabase::new();
    let (libpath, pkg) = make_installed_package(&mut db, "dplyr");
    db.library_roots().set_roots(&mut db).to(vec![libpath]);

    assert_eq!(db.package_by_name("dplyr"), Some(pkg));
}

#[test]
fn package_by_name_workspace_shadows_installed() {
    let mut db = OakDatabase::new();
    let (workspace, workspace_pkg) = make_workspace_package(&mut db, "rlang");
    let (libpath, _installed_pkg) = make_installed_package(&mut db, "rlang");
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);
    db.library_roots().set_roots(&mut db).to(vec![libpath]);

    assert_eq!(db.package_by_name("rlang"), Some(workspace_pkg));
}

#[test]
fn package_by_name_returns_none_when_absent() {
    let db = OakDatabase::new();
    assert_eq!(db.package_by_name("ggplot2"), None);
}

#[test]
fn root_scripts_round_trips_through_a_tracked_query() {
    // Exercises `Root.scripts: Vec<File>` as input to a tracked-query
    // return, confirming the salsa machinery picks up changes to the
    // scripts list and to which workspace root is registered.
    #[salsa::tracked]
    fn first(db: &dyn Db) -> Option<File> {
        for root in db.workspace_roots().roots(db) {
            if let Some(&file) = root.scripts(db).first() {
                return Some(file);
            }
        }
        None
    }

    let mut db = OakDatabase::new();
    assert_eq!(first(&db), None);

    let root = workspace_root(&db, "workspace");
    db.workspace_roots().set_roots(&mut db).to(vec![root]);
    assert_eq!(first(&db), None);

    let file = make_script(&mut db, "a.R");
    root.set_scripts(&mut db).to(vec![file]);
    assert_eq!(first(&db), Some(file));

    root.set_scripts(&mut db).to(vec![]);
    assert_eq!(first(&db), None);
}
