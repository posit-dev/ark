use salsa::Setter;

use crate::file_reader::EmptyFileReader;
use crate::test_path::file_path;
use crate::tests::test_db::library_root;
use crate::tests::test_db::workspace_root;
use crate::Db;
use crate::DbInputs;
use crate::File;
use crate::FileRevision;
use crate::OakDatabase;
use crate::Package;
use crate::PackageFiles;
use crate::Root;
use crate::SourceDb;

fn make_workspace_package(db: &mut OakDatabase, name: &str) -> (Root, Package) {
    let root = workspace_root(db, &format!("workspace/{name}"));
    let pkg = Package::new(
        db,
        file_path(&format!("workspace/{name}/DESCRIPTION")),
        name.to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        PackageFiles::default(),
        Vec::new(),
    );
    root.set_packages(db).to(vec![pkg]);
    (root, pkg)
}

fn make_installed_package(db: &mut OakDatabase, name: &str) -> (Root, Package) {
    let root = library_root(db, &format!("libs/{name}"));
    let pkg = Package::new(
        db,
        file_path(&format!("libs/{name}/DESCRIPTION")),
        name.to_string(),
        FileRevision::zero(),
        FileRevision::zero(),
        None,
        None,
        PackageFiles::default(),
        Vec::new(),
    );
    root.set_packages(db).to(vec![pkg]);
    (root, pkg)
}

fn make_script(db: &mut OakDatabase, name: &str) -> File {
    File::new(db, file_path(name), FileRevision::zero(), None, None)
}

#[test]
fn test_package_by_name_finds_workspace_package() {
    let mut db = OakDatabase::with_file_reader(EmptyFileReader);
    let (root, pkg) = make_workspace_package(&mut db, "rlang");
    db.workspace_roots().set_roots(&mut db).to(vec![root]);

    assert_eq!(db.package_by_name("rlang"), Some(pkg));
}

#[test]
fn test_package_by_name_falls_back_to_installed() {
    let mut db = OakDatabase::with_file_reader(EmptyFileReader);
    let (libpath, pkg) = make_installed_package(&mut db, "dplyr");
    db.library_roots().set_roots(&mut db).to(vec![libpath]);

    assert_eq!(db.package_by_name("dplyr"), Some(pkg));
}

#[test]
fn test_package_by_name_workspace_shadows_installed() {
    let mut db = OakDatabase::with_file_reader(EmptyFileReader);
    let (workspace, workspace_pkg) = make_workspace_package(&mut db, "rlang");
    let (libpath, _installed_pkg) = make_installed_package(&mut db, "rlang");
    db.workspace_roots().set_roots(&mut db).to(vec![workspace]);
    db.library_roots().set_roots(&mut db).to(vec![libpath]);

    assert_eq!(db.package_by_name("rlang"), Some(workspace_pkg));
}

#[test]
fn test_package_by_name_returns_none_when_absent() {
    let db = OakDatabase::with_file_reader(EmptyFileReader);
    assert_eq!(db.package_by_name("ggplot2"), None);
}

#[test]
fn test_root_scripts_round_trips_through_a_tracked_query() {
    // Exercises `Root.scripts: Vec<File>` as input to a tracked-query
    // return, confirming the salsa machinery picks up changes to the
    // scripts list and to which workspace root is registered.
    #[salsa::tracked(returns(copy))]
    fn first(db: &dyn Db) -> Option<File> {
        for root in db.workspace_roots().roots(db) {
            if let Some(&file) = root.scripts(db).first() {
                return Some(file);
            }
        }
        None
    }

    let mut db = OakDatabase::with_file_reader(EmptyFileReader);
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

#[test]
fn test_package_membership_keeps_sources_before_generated_files() {
    let mut db = OakDatabase::with_file_reader(EmptyFileReader);
    let (_root, pkg) = make_installed_package(&mut db, "base");
    let a = make_script(&mut db, "a.R");
    let b = make_script(&mut db, "b.R");
    let c = make_script(&mut db, "c.R");
    let stub = make_script(&mut db, "stubs.R");

    pkg.set_generated_files(&mut db, vec![stub]);
    pkg.set_source_files(&mut db, vec![a]);
    assert_eq!(pkg.files(&db), [a, stub]);

    pkg.add_source_file(&mut db, b);
    assert_eq!(pkg.files(&db), [a, b, stub]);

    let before = salsa::plumbing::current_revision(&db);
    pkg.add_source_file(&mut db, b);
    pkg.set_source_files(&mut db, vec![a, b]);
    pkg.set_generated_files(&mut db, vec![stub]);
    assert!(!pkg.remove_file(&mut db, c));
    assert_eq!(salsa::plumbing::current_revision(&db), before);

    // Removing a source must update the boundary, or replacing sources drops the stub.
    assert!(pkg.remove_file(&mut db, a));
    pkg.set_source_files(&mut db, vec![a]);
    assert_eq!(pkg.files(&db), [a, stub]);

    assert!(pkg.remove_file(&mut db, stub));
    pkg.set_generated_files(&mut db, vec![stub]);
    assert_eq!(pkg.files(&db), [a, stub]);

    pkg.clear_files(&mut db);
    assert!(pkg.files(&db).is_empty());
    pkg.set_generated_files(&mut db, vec![stub]);
    assert_eq!(pkg.files(&db), [stub]);
}
