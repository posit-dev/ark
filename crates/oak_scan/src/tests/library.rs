//! Initial library scans and watcher-driven rescans share [`list_library()`]
//! and [`set_installed_packages()`]. These tests cover listing and entity reuse.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use aether_path::FilePath;
use filetime::set_file_mtime;
use filetime::FileTime;
use oak_db::DbInputs;
use oak_db::FileRevision;
use oak_db::OakDatabase;
use oak_db::Root;
use oak_db::SourceDb;
use oak_package_metadata::tests::mark_installed;

use crate::library::list_library;
use crate::library::set_installed_packages;
use crate::library::InstalledPackageEntry;
use crate::DbScan;

fn write_installed_package(lib: &Path, name: &str, version: &str) {
    let dir = lib.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("DESCRIPTION"),
        format!("Package: {name}\nVersion: {version}\n"),
    )
    .unwrap();
    mark_installed(&dir);
}

fn set_mtime(path: &Path, seconds: i64) {
    set_file_mtime(path, FileTime::from_unix_time(seconds, 0)).unwrap();
}

fn revision(seconds: i64) -> FileRevision {
    FileRevision::from(FileTime::from_unix_time(seconds, 0))
}

fn library_root(db: &OakDatabase, lib: &Path) -> Root {
    let roots = db.library_roots().roots(db);
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].path(db).as_path().unwrap().as_std_path(), lib);
    roots[0]
}

fn rescan(db: &mut OakDatabase, root: Root) {
    rescan_with_editor_owned(db, root, &HashSet::new());
}

fn rescan_with_editor_owned(db: &mut OakDatabase, root: Root, editor_owned: &HashSet<FilePath>) {
    let path = root.path(db).as_path().unwrap().as_std_path().to_path_buf();
    set_installed_packages(db, root, list_library(&path), Some(editor_owned));
}

fn package_names(db: &OakDatabase, root: Root) -> Vec<String> {
    root.packages(db)
        .iter()
        .map(|package| package.name(db).clone())
        .collect()
}

#[test]
fn test_list_library_lists_only_package_directories() {
    let lib = tempfile::tempdir().unwrap();
    let pkg_dir = lib.path().join("pkg");
    write_installed_package(lib.path(), "pkg", "1.0.0");
    fs::write(pkg_dir.join("NAMESPACE"), "export(f)\n").unwrap();
    fs::write(pkg_dir.join("INDEX"), "f     Do things\n").unwrap();
    set_mtime(&pkg_dir.join("DESCRIPTION"), 1_000);
    set_mtime(&pkg_dir.join("NAMESPACE"), 2_000);
    set_mtime(&pkg_dir.join("INDEX"), 3_000);

    // Install locks, temporary files, and source directories (a `DESCRIPTION`
    // but no `Meta/package.rds`) must not count as installed packages.
    fs::create_dir_all(lib.path().join("00LOCK-pkg").join("00new")).unwrap();
    fs::write(lib.path().join("file1a2b3c"), "").unwrap();
    fs::create_dir_all(lib.path().join("src")).unwrap();
    fs::write(
        lib.path().join("src").join("DESCRIPTION"),
        "Package: src\nVersion: 1.0.0\n",
    )
    .unwrap();

    let entries = list_library(lib.path());

    assert_eq!(entries, vec![InstalledPackageEntry {
        name: "pkg".to_string(),
        description_path: FilePath::from_path_buf(pkg_dir.join("DESCRIPTION")).unwrap(),
        description_revision: revision(1_000),
        namespace_revision: revision(2_000),
        index_revision: revision(3_000),
    }]);
}

#[test]
fn test_list_library_missing_directory_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(list_library(&tmp.path().join("missing")).is_empty());
}

#[test]
fn test_rescan_adds_installed_package_and_keeps_existing_entity() {
    let lib = tempfile::tempdir().unwrap();
    write_installed_package(lib.path(), "a", "1.0.0");
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    let a = db.package_by_name("a").unwrap();

    write_installed_package(lib.path(), "b", "1.0.0");
    rescan(&mut db, root);

    assert_eq!(package_names(&db, root), vec!["a", "b"]);
    assert_eq!(db.package_by_name("a"), Some(a));
    assert!(db.package_by_name("b").is_some());
}

#[test]
fn test_rescan_drops_removed_package_without_staling_it() {
    let lib = tempfile::tempdir().unwrap();
    write_installed_package(lib.path(), "a", "1.0.0");
    write_installed_package(lib.path(), "b", "1.0.0");
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    let b = db.package_by_name("b").unwrap();

    fs::remove_dir_all(lib.path().join("b")).unwrap();
    rescan(&mut db, root);

    assert_eq!(package_names(&db, root), vec!["a"]);
    assert!(db.package_by_name("b").is_none());
    assert!(db.stale_root().packages(&db).is_empty());

    // A reinstall after removal was observed must get a new entity, so the LSP
    // source scheduler does not treat it as an already-fetched package.
    write_installed_package(lib.path(), "b", "2.0.0");
    rescan(&mut db, root);

    let reinstalled = db.package_by_name("b").unwrap();
    assert_ne!(reinstalled, b);
    assert_eq!(reinstalled.version(&db), &Some("2.0.0".to_string()));
}

#[test]
fn test_rescan_in_place_reinstall_refreshes_revisions_on_same_entity() {
    let lib = tempfile::tempdir().unwrap();
    let pkg_dir = lib.path().join("pkg");
    write_installed_package(lib.path(), "pkg", "1.0.0");
    fs::write(pkg_dir.join("NAMESPACE"), "export(old)\n").unwrap();
    set_mtime(&pkg_dir.join("DESCRIPTION"), 1_000);
    set_mtime(&pkg_dir.join("NAMESPACE"), 1_000);
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    let pkg = db.package_by_name("pkg").unwrap();
    assert_eq!(pkg.version(&db), &Some("1.0.0".to_string()));
    assert_eq!(pkg.namespace(&db).exports.to_vec(), vec!["old"]);

    // No listing observes the package's absence, as when replacement events
    // from a binary install or renv relink arrive in one watcher batch.
    fs::remove_dir_all(&pkg_dir).unwrap();
    write_installed_package(lib.path(), "pkg", "2.0.0");
    fs::write(pkg_dir.join("NAMESPACE"), "export(new)\n").unwrap();
    set_mtime(&pkg_dir.join("DESCRIPTION"), 2_000);
    set_mtime(&pkg_dir.join("NAMESPACE"), 2_000);
    rescan(&mut db, root);

    assert_eq!(db.package_by_name("pkg"), Some(pkg));
    assert_eq!(pkg.description_revision(&db), revision(2_000));
    assert_eq!(pkg.namespace_revision(&db), revision(2_000));
    assert_eq!(pkg.version(&db), &Some("2.0.0".to_string()));
    assert_eq!(pkg.namespace(&db).exports.to_vec(), vec!["new"]);
}

#[test]
fn test_rescan_bumps_index_revision() {
    let lib = tempfile::tempdir().unwrap();
    let index = lib.path().join("pkg").join("INDEX");
    write_installed_package(lib.path(), "pkg", "1.0.0");
    fs::write(&index, "old     Old topic\n").unwrap();
    set_mtime(&index, 1_000);
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    let pkg = db.package_by_name("pkg").unwrap();
    assert_eq!(pkg.index(&db).as_ref().unwrap().names(), &["old"]);

    fs::write(&index, "new     New topic\n").unwrap();
    set_mtime(&index, 2_000);
    rescan(&mut db, root);

    assert_eq!(pkg.index_revision(&db), Some(revision(2_000)));
    assert_eq!(pkg.index(&db).as_ref().unwrap().names(), &["new"]);
}

#[test]
fn test_unchanged_rescan_keeps_sources_and_writes_nothing() {
    let lib = tempfile::tempdir().unwrap();
    // Creation order differs from the expected alphabetical listing.
    write_installed_package(lib.path(), "pkg", "1.0.0");
    write_installed_package(lib.path(), "aaa", "1.0.0");
    write_installed_package(lib.path(), "zzz", "1.0.0");
    let sources = tempfile::tempdir().unwrap();
    fs::write(sources.path().join("f.R"), "f <- function() 1\n").unwrap();
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    assert_eq!(package_names(&db, root), vec!["aaa", "pkg", "zzz"]);
    let pkg = db.package_by_name("pkg").unwrap();
    db.set_package_sources(pkg, sources.path());
    let files = pkg.files(&db).clone();
    assert_eq!(files.len(), 1);

    let before = salsa::plumbing::current_revision(&db);
    rescan(&mut db, root);

    assert_eq!(salsa::plumbing::current_revision(&db), before);
    assert_eq!(package_names(&db, root), vec!["aaa", "pkg", "zzz"]);
    assert_eq!(pkg.files(&db), &files);
    assert_eq!(files[0].package(&db), Some(pkg));
}

#[test]
fn test_rescan_moves_removed_package_sources_to_stale() {
    let lib = tempfile::tempdir().unwrap();
    write_installed_package(lib.path(), "pkg", "1.0.0");
    let sources = tempfile::tempdir().unwrap();
    fs::write(sources.path().join("f.R"), "f <- function() 1\n").unwrap();
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    let pkg = db.package_by_name("pkg").unwrap();
    db.set_package_sources(pkg, sources.path());
    let file = pkg.files(&db)[0];

    fs::remove_dir_all(lib.path().join("pkg")).unwrap();
    rescan(&mut db, root);

    assert!(root.packages(&db).is_empty());
    assert!(pkg.files(&db).is_empty());
    assert_eq!(file.package(&db), None);
    assert!(db.stale_root().files(&db).contains(&file));
    assert!(db.stale_root().packages(&db).is_empty());

    // The sources of the reinstalled package reuse the stale `File` rather
    // than minting a second one at the same path.
    write_installed_package(lib.path(), "pkg", "2.0.0");
    rescan(&mut db, root);
    let reinstalled = db.package_by_name("pkg").unwrap();
    assert_ne!(reinstalled, pkg);
    db.set_package_sources(reinstalled, sources.path());

    assert_eq!(reinstalled.files(&db), &vec![file]);
    assert_eq!(file.package(&db), Some(reinstalled));
    assert!(!db.stale_root().files(&db).contains(&file));
}

#[test]
fn test_rescan_moves_removed_package_editor_owned_sources_to_orphan() {
    let lib = tempfile::tempdir().unwrap();
    write_installed_package(lib.path(), "pkg", "1.0.0");
    let sources = tempfile::tempdir().unwrap();
    fs::write(sources.path().join("f.R"), "f <- function() 1\n").unwrap();
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    let pkg = db.package_by_name("pkg").unwrap();
    db.set_package_sources(pkg, sources.path());
    let file = pkg.files(&db)[0];
    let path = file.path(&db).clone();
    db.upsert_editor(path.clone(), String::from("f <- function() 2\n"));

    fs::remove_dir_all(lib.path().join("pkg")).unwrap();
    rescan_with_editor_owned(&mut db, root, &HashSet::from([path.clone()]));

    assert!(pkg.files(&db).is_empty());
    assert_eq!(file.package(&db), None);
    assert!(db.orphan_root().files(&db).contains(&file));
    assert!(!db.stale_root().files(&db).contains(&file));
    assert_eq!(db.file_by_path(&path), Some(file));
}

#[test]
fn test_rescan_upgrade_moves_sources_to_stale() {
    let lib = tempfile::tempdir().unwrap();
    let description = lib.path().join("pkg").join("DESCRIPTION");
    // `Collate` excludes `script.R`, putting it in `scripts` so the test covers
    // rehoming both package source lists.
    let write_description = |version: &str| {
        fs::create_dir_all(lib.path().join("pkg")).unwrap();
        fs::write(
            &description,
            format!("Package: pkg\nVersion: {version}\nCollate: 'f.R'\n"),
        )
        .unwrap();
        mark_installed(&lib.path().join("pkg"));
    };
    write_description("1.0.0");
    set_mtime(&description, 1_000);
    let sources = tempfile::tempdir().unwrap();
    fs::write(sources.path().join("f.R"), "f <- function() 1\n").unwrap();
    fs::write(sources.path().join("script.R"), "1 + 1\n").unwrap();
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    let pkg = db.package_by_name("pkg").unwrap();
    db.set_package_sources(pkg, sources.path());
    let file = pkg.files(&db)[0];
    let script = pkg.scripts(&db)[0];

    write_description("2.0.0");
    set_mtime(&description, 2_000);
    rescan(&mut db, root);

    assert_eq!(db.package_by_name("pkg"), Some(pkg));
    assert_eq!(root.packages(&db), &vec![pkg]);
    assert!(pkg.files(&db).is_empty());
    assert!(pkg.scripts(&db).is_empty());
    assert_eq!(file.package(&db), None);
    assert_eq!(script.package(&db), None);
    assert_eq!(db.stale_root().files(&db), &HashSet::from([file, script]));
    assert!(db.stale_root().packages(&db).is_empty());

    // Refetched sources at the same paths must reuse the stale `File` entities.
    db.set_package_sources(pkg, sources.path());
    assert_eq!(pkg.files(&db), &vec![file]);
    assert_eq!(pkg.scripts(&db), &vec![script]);
    assert_eq!(file.package(&db), Some(pkg));
    assert!(db.stale_root().files(&db).is_empty());
}

#[test]
fn test_rescan_upgrade_moves_editor_owned_sources_to_orphan() {
    let lib = tempfile::tempdir().unwrap();
    let description = lib.path().join("pkg").join("DESCRIPTION");
    write_installed_package(lib.path(), "pkg", "1.0.0");
    set_mtime(&description, 1_000);
    let sources = tempfile::tempdir().unwrap();
    fs::write(sources.path().join("f.R"), "f <- function() 1\n").unwrap();
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    let pkg = db.package_by_name("pkg").unwrap();
    db.set_package_sources(pkg, sources.path());
    let file = pkg.files(&db)[0];
    let path = file.path(&db).clone();
    db.upsert_editor(path.clone(), String::from("f <- function() 2\n"));

    write_installed_package(lib.path(), "pkg", "2.0.0");
    set_mtime(&description, 2_000);
    rescan_with_editor_owned(&mut db, root, &HashSet::from([path.clone()]));

    assert_eq!(db.package_by_name("pkg"), Some(pkg));
    assert!(pkg.files(&db).is_empty());
    assert_eq!(file.package(&db), None);
    assert_eq!(db.orphan_root().files(&db), &HashSet::from([file]));
    assert!(db.stale_root().files(&db).is_empty());
    assert_eq!(db.file_by_path(&path), Some(file));
}

#[test]
fn test_rescan_unchanged_description_revision_keeps_sources() {
    let lib = tempfile::tempdir().unwrap();
    let pkg_dir = lib.path().join("pkg");
    write_installed_package(lib.path(), "pkg", "1.0.0");
    fs::write(pkg_dir.join("NAMESPACE"), "export(f)\n").unwrap();
    set_mtime(&pkg_dir.join("DESCRIPTION"), 1_000);
    set_mtime(&pkg_dir.join("NAMESPACE"), 1_000);
    let sources = tempfile::tempdir().unwrap();
    fs::write(sources.path().join("f.R"), "f <- function() 1\n").unwrap();
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    let pkg = db.package_by_name("pkg").unwrap();
    db.set_package_sources(pkg, sources.path());
    let files = pkg.files(&db).clone();

    // A `NAMESPACE` revision change must not discard sources when the
    // `DESCRIPTION` revision is unchanged.
    set_mtime(&pkg_dir.join("NAMESPACE"), 2_000);
    rescan(&mut db, root);

    assert_eq!(pkg.namespace_revision(&db), revision(2_000));
    assert_eq!(pkg.files(&db), &files);
    assert_eq!(files[0].package(&db), Some(pkg));
    assert!(db.stale_root().files(&db).is_empty());
}

#[test]
fn test_rescan_renamed_package_dir_mints_new_package() {
    let lib = tempfile::tempdir().unwrap();
    write_installed_package(lib.path(), "old", "1.0.0");
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    let old = db.package_by_name("old").unwrap();

    // Identity is the `DESCRIPTION` path, which the rename changes.
    fs::rename(lib.path().join("old"), lib.path().join("new")).unwrap();
    rescan(&mut db, root);

    let new = db.package_by_name("new").unwrap();
    assert_ne!(new, old);
    assert_eq!(root.packages(&db), &vec![new]);
    assert!(db.package_by_name("old").is_none());
}
