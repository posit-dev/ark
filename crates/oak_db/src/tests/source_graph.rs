use oak_package_metadata::namespace::Namespace;
use salsa::Setter;

use crate::package_by_name;
use crate::tests::test_db::file_url;
use crate::tests::test_db::library_root;
use crate::tests::test_db::workspace_root;
use crate::tests::test_db::TestDb;
use crate::Db;
use crate::File;
use crate::FileOwner;
use crate::Name;
use crate::Package;
use crate::Root;
use crate::Script;

fn name<'db>(db: &'db TestDb, text: &str) -> Name<'db> {
    Name::new(db, text)
}

fn make_workspace_package(db: &TestDb, name: &str) -> (Root, Package) {
    let root = workspace_root(db, &format!("workspace/{name}"));
    let pkg = Package::new(
        db,
        root,
        name.to_string(),
        None,
        Namespace::default(),
        Vec::new(),
    );
    (root, pkg)
}

fn make_installed_package(db: &TestDb, name: &str) -> (Root, Package) {
    let root = library_root(db, &format!("libs/{name}"));
    let pkg = Package::new(
        db,
        root,
        name.to_string(),
        Some("1.0.0".to_string()),
        Namespace::default(),
        Vec::new(),
    );
    (root, pkg)
}

fn make_script(db: &TestDb, root: Root, name: &str) -> Script {
    let file = File::new(db, file_url(name), String::new());
    Script::new(db, root, file)
}

#[test]
fn package_by_name_finds_workspace_package() {
    let mut db = TestDb::new();
    let (root, pkg) = make_workspace_package(&db, "rlang");
    root.set_packages(&mut db).to(vec![pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(package_by_name(&db, name(&db, "rlang")), Some(pkg));
}

#[test]
fn package_by_name_falls_back_to_installed() {
    let mut db = TestDb::new();
    let (libpath, pkg) = make_installed_package(&db, "dplyr");
    libpath.set_packages(&mut db).to(vec![pkg]);
    db.library_roots().set_roots(&mut db).to(vec![libpath]);

    assert_eq!(package_by_name(&db, name(&db, "dplyr")), Some(pkg));
}

#[test]
fn package_by_name_workspace_shadows_installed() {
    let mut db = TestDb::new();
    let (workspace, workspace_pkg) = make_workspace_package(&db, "rlang");
    let (libpath, installed_pkg) = make_installed_package(&db, "rlang");
    workspace.set_packages(&mut db).to(vec![workspace_pkg]);
    libpath.set_packages(&mut db).to(vec![installed_pkg]);
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);
    db.library_roots().set_roots(&mut db).to(vec![libpath]);

    assert_eq!(
        package_by_name(&db, name(&db, "rlang")),
        Some(workspace_pkg),
    );
}

#[test]
fn package_by_name_returns_none_when_absent() {
    let db = TestDb::new();
    assert_eq!(package_by_name(&db, name(&db, "ggplot2")), None);
}

#[test]
fn source_node_round_trips_through_a_tracked_query() {
    // SourceNode is a plain enum over Salsa input ids; this exercises
    // it as a tracked-query return type, confirming the auto-derived
    // Update / equality machinery works.
    #[salsa::tracked]
    fn first(db: &dyn Db) -> Option<FileOwner> {
        for root in db.workspace_roots().roots(db) {
            if let Some(&script) = root.scripts(db).first() {
                return Some(FileOwner::Script(script));
            }
            if let Some(&package) = root.packages(db).first() {
                return Some(FileOwner::Package(package));
            }
        }
        None
    }

    let mut db = TestDb::new();
    assert_eq!(first(&db), None);

    let root = workspace_root(&db, "workspace");
    db.workspace_roots().set_roots(&mut db).to(vec![root]);
    assert_eq!(first(&db), None);

    let script = make_script(&db, root, "a.R");
    root.set_scripts(&mut db).to(vec![script]);
    assert_eq!(first(&db), Some(FileOwner::Script(script)));

    root.set_scripts(&mut db).to(vec![]);
    let (pkg_root, package) = make_workspace_package(&db, "rlang");
    pkg_root.set_packages(&mut db).to(vec![package]);
    db.workspace_roots()
        .set_roots(&mut db)
        .to(vec![root, pkg_root]);
    assert_eq!(first(&db), Some(FileOwner::Package(package)));
}
