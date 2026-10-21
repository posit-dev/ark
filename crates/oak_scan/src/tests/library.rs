//! Library listing and rescans, nested-root resolution through [`RootExt`],
//! and open-buffer visibility across library eviction and rescans.
//!
//! Initial library scans and watcher-driven rescans share [`list_library()`]
//! and [`set_installed_packages()`].

use std::fs;
use std::path::Path;
use std::slice;

use aether_path::FilePath;
use filetime::set_file_mtime;
use filetime::FileTime;
use oak_db::all_known_files;
use oak_db::DbInputs;
use oak_db::FileRevision;
use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::Root;
use oak_db::RootKind;
use oak_db::SourceDb;
use salsa::Setter;
use url::Url;

use crate::inputs::FileEntry;
use crate::inputs::RootExt;
use crate::library::list_library;
use crate::library::InstalledPackageEntry;
use crate::scheduler::drain_scheduler;
use crate::DbScan;
use crate::ScanScheduler;

// --- nested-root resolution (longest-path-wins) ---
//
// The library scanner walks depth-1 so it can't naturally produce two
// library roots that both claim the same DESCRIPTION (a nested library
// root at `/lib/foo` would walk children of `/lib/foo`, not
// `/lib/foo/DESCRIPTION` itself). These tests exercise `RootExt::set_package`
// directly with hand-built `Root` entities so they cover the shared
// resolution logic that PR 12's workspace scanner -- which walks any depth
// and is the realistic trigger -- depends on.

fn file_path(s: &str) -> FilePath {
    FilePath::from_url(&Url::parse(&format!("file://{s}")).unwrap())
}

fn empty_library_root(db: &OakDatabase, path: &str) -> Root {
    Root::new(
        db,
        file_path(path),
        RootKind::Library,
        vec![],
        vec![],
        vec![],
    )
}

/// Stash `pkg` in `root.packages` and register `root` on
/// `library_roots`, mirroring what the library scanner does after
/// `set_package` returns. The identity map finds the package without this
/// step. It matters for visibility to analysis and for `root_by_package()`.
fn register_package(db: &mut OakDatabase, root: Root, pkg: Package) {
    root.set_packages(db).to(vec![pkg]);
    let mut roots = db.library_roots().roots(db).clone();
    if !roots.contains(&root) {
        roots.push(root);
    }
    db.library_roots().set_roots(db).to(roots);
}

#[test]
fn test_set_package_longer_root_wins_after_shorter_claims_first() {
    let mut db = OakDatabase::new();
    let short = empty_library_root(&db, "/lib");
    let long = empty_library_root(&db, "/lib/sub");
    let desc_path = file_path("/lib/sub/DESCRIPTION");

    let p1 = short.set_package(
        &mut db,
        desc_path.clone(),
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        None,
        Vec::new(),
        Vec::new(),
    );
    register_package(&mut db, short, p1);
    assert_eq!(db.root_by_package(p1), Some(short));

    let p2 = long.set_package(
        &mut db,
        desc_path,
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        None,
        Vec::new(),
        Vec::new(),
    );
    register_package(&mut db, long, p2);
    // Same entity; now in both roots' `packages`. `root_by_package` prefers
    // the longer (more specific) root.
    assert_eq!(p1, p2);
    assert_eq!(db.root_by_package(p1), Some(long));
}

#[test]
fn test_set_package_shorter_root_does_not_steal_from_longer() {
    let mut db = OakDatabase::new();
    let short = empty_library_root(&db, "/lib");
    let long = empty_library_root(&db, "/lib/sub");
    let desc_path = file_path("/lib/sub/DESCRIPTION");

    let p1 = long.set_package(
        &mut db,
        desc_path.clone(),
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        None,
        Vec::new(),
        Vec::new(),
    );
    register_package(&mut db, long, p1);
    assert_eq!(db.root_by_package(p1), Some(long));

    let p2 = short.set_package(
        &mut db,
        desc_path,
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        None,
        Vec::new(),
        Vec::new(),
    );
    register_package(&mut db, short, p2);
    // Same entity; now in both roots' `packages`. `root_by_package` keeps the
    // longer root as the owner.
    assert_eq!(p1, p2);
    assert_eq!(db.root_by_package(p1), Some(long));
}

#[test]
fn test_all_files_emits_shared_file_once_under_deepest_root() {
    let mut db = OakDatabase::new();
    let short = empty_library_root(&db, "/lib");
    let long = empty_library_root(&db, "/lib/sub");
    let desc_path = file_path("/lib/sub/DESCRIPTION");
    let r_path = file_path("/lib/sub/R/a.R");
    let files = vec![FileEntry {
        path: r_path,
        revision: oak_db::FileRevision::zero(),
    }];

    // Same DESCRIPTION scanned from both roots reuses one `Package`, so
    // both roots' `packages` vecs list it and its files are reachable from
    // both. The deepest root owns them.
    let p1 = short.set_package(
        &mut db,
        desc_path.clone(),
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        None,
        files.clone(),
        Vec::new(),
    );
    register_package(&mut db, short, p1);
    let p2 = long.set_package(
        &mut db,
        desc_path,
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        None,
        files,
        Vec::new(),
    );
    register_package(&mut db, long, p2);
    assert_eq!(p1, p2);

    let file = p1.files(&db)[0];
    assert_eq!(file.root(&db), Some(long));
    assert_eq!(all_known_files(&db), &vec![file]);
}

#[test]
fn test_upsert_re_promotes_editor_owned_file_from_orphan() {
    // An editor-open file in `OrphanRoot` should move into `pkg.files` when a
    // scan of the package's root lists it. Same `File` entity, editor
    // contents preserved (the scan's disk snapshot doesn't overwrite),
    // and the orphan reference is cleaned up so the orphan-placement
    // invariant (`package == None`) stays honest.

    let mut db = OakDatabase::new();

    // Editor opens the file before any scan -> orphan.
    let r_path = file_path("/lib/pkg/R/a.R");
    let file = db.upsert_editor(r_path.clone(), "editor content".to_string());
    assert!(db.orphan_root().files(&db).contains(&file));
    assert_eq!(file.package(&db), None);

    // Now a library scan picks up the same URL as part of a package.
    let lib = empty_library_root(&db, "/lib");
    let pkg = lib.set_package(
        &mut db,
        file_path("/lib/pkg/DESCRIPTION"),
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        None,
        vec![FileEntry {
            path: r_path.clone(),
            revision: oak_db::FileRevision::zero(),
        }],
        Vec::new(),
    );

    // Same `File` entity, editor content preserved, package backpointer set.
    let pkg_file = pkg.files(&db)[0];
    assert_eq!(pkg_file, file);
    assert_eq!(file.source_text(&db), "editor content");
    assert_eq!(file.package(&db), Some(pkg));

    // Orphan reference cleaned up.
    assert!(!db.orphan_root().files(&db).contains(&file));
}

#[test]
fn test_set_package_after_eviction_changes_owning_root() {
    // Package identity is independent of root identity. Re-adding its path
    // under a fresh `Root` must make `root_by_package()` resolve to that root.
    let mut db = OakDatabase::new();
    let old = empty_library_root(&db, "/lib");
    let new = empty_library_root(&db, "/lib");
    assert_ne!(old, new);
    let desc_path = file_path("/lib/pkg/DESCRIPTION");

    let p1 = old.set_package(
        &mut db,
        desc_path.clone(),
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        None,
        Vec::new(),
        Vec::new(),
    );
    register_package(&mut db, old, p1);
    assert_eq!(db.root_by_package(p1), Some(old));

    old.evict(&mut db);
    db.library_roots().set_roots(&mut db).to(vec![]);
    assert_eq!(db.root_by_package(p1), None);

    let p2 = new.set_package(
        &mut db,
        desc_path,
        "pkg".to_string(),
        oak_db::FileRevision::zero(),
        oak_db::FileRevision::zero(),
        None,
        Vec::new(),
        Vec::new(),
    );
    register_package(&mut db, new, p2);
    assert_eq!(p1, p2);
    assert_eq!(db.root_by_package(p1), Some(new));
}

#[test]
fn test_set_library_paths_eviction_keeps_open_buffer_visible() {
    // Removing a library path must keep open package sources visible as
    // standalone files in `OrphanRoot`, even when the sources live elsewhere.
    let lib = tempfile::tempdir().unwrap();
    write_package(&lib.path().join("pkg"), "pkg", &[]);
    let sources = tempfile::tempdir().unwrap();
    fs::write(sources.path().join("a.R"), "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();

    db.set_library_paths(&[lib.path().to_path_buf()]);
    let pkg = db.package_by_name("pkg").unwrap();
    db.set_package_sources(pkg, sources.path());
    let path = FilePath::from_path_buf(sources.path().join("a.R")).unwrap();
    let file = db.upsert_editor(path.clone(), "edited <- 1\n".to_string());
    assert_eq!(pkg.files(&db), &[file]);

    db.set_library_paths(&[]);

    assert_eq!(db.file_by_path(&path), Some(file));
    assert!(db.orphan_root().files(&db).contains(&file));
    assert_eq!(file.package(&db), None);
    assert_eq!(file.source_text(&db), "edited <- 1\n");
}

fn write_package(dir: &Path, name: &str, r_files: &[(&str, &str)]) {
    fs::create_dir_all(dir.join("R")).unwrap();
    fs::write(
        dir.join("DESCRIPTION"),
        format!("Package: {name}\nVersion: 1.0.0\n"),
    )
    .unwrap();
    for (basename, contents) in r_files {
        fs::write(dir.join("R").join(basename), contents).unwrap();
    }
}

// --- library listing and rescans ---

fn write_installed_package(lib: &Path, name: &str, version: &str) {
    let dir = lib.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("DESCRIPTION"),
        format!("Package: {name}\nVersion: {version}\n"),
    )
    .unwrap();
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

/// Rescan `root` the way a watcher event on one of its direct entries does,
/// through the scheduler's public writers.
fn rescan(db: &mut OakDatabase, root: Root) {
    let lib = root.path(db).as_path().unwrap().as_std_path().to_path_buf();
    let entry = FilePath::from_path_buf(lib.join("entry")).unwrap();
    let mut scheduler = ScanScheduler::new();
    let requests = scheduler.apply_watcher_events(db, vec![entry]);
    assert_eq!(requests.len(), 1);
    drain_scheduler(db, &mut scheduler, requests);
    assert!(!scheduler.has_pending_scans());
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

    // Install locks without `DESCRIPTION` and temporary files must not count
    // as installed packages.
    fs::create_dir_all(lib.path().join("00LOCK-pkg").join("00new")).unwrap();
    fs::write(lib.path().join("file1a2b3c"), "").unwrap();

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
fn test_rescan_drops_removed_package_and_reinstall_reuses_entity() {
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

    // `IdentityMap` keeps one `Package` per `DESCRIPTION` path, so a reinstall
    // reuses the entity with refreshed metadata.
    write_installed_package(lib.path(), "b", "2.0.0");
    rescan(&mut db, root);

    assert_eq!(db.package_by_name("b"), Some(b));
    assert_eq!(b.version(&db), &Some("2.0.0".to_string()));
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
    let files = pkg.files(&db).to_vec();
    assert_eq!(files.len(), 1);

    let before = salsa::plumbing::current_revision(&db);
    rescan(&mut db, root);

    assert_eq!(salsa::plumbing::current_revision(&db), before);
    assert_eq!(package_names(&db, root), vec!["aaa", "pkg", "zzz"]);
    assert_eq!(pkg.files(&db), files.as_slice());
    assert_eq!(files[0].package(&db), Some(pkg));
}

#[test]
fn test_rescan_clears_removed_package_sources() {
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

    fs::remove_dir_all(lib.path().join("pkg")).unwrap();
    rescan(&mut db, root);

    assert!(root.packages(&db).is_empty());
    assert!(pkg.files(&db).is_empty());
    assert_eq!(file.package(&db), None);
    assert!(db.file_by_path(&path).is_none());

    // The sources of the reinstalled package reuse the same `File` rather
    // than minting a second one at the same path.
    write_installed_package(lib.path(), "pkg", "2.0.0");
    rescan(&mut db, root);
    let reinstalled = db.package_by_name("pkg").unwrap();
    db.set_package_sources(reinstalled, sources.path());

    assert_eq!(reinstalled.files(&db), &vec![file]);
    assert_eq!(file.package(&db), Some(reinstalled));
    assert_eq!(db.file_by_path(&path), Some(file));
}

#[test]
fn test_rescan_removal_resets_sources_revision() {
    // A reinstall with the same `DESCRIPTION` revision, as when renv relinks
    // the same cache entry, revives the `Package` with empty lists, so it must
    // not look fetched.
    let lib = tempfile::tempdir().unwrap();
    let pkg_dir = lib.path().join("pkg");
    write_installed_package(lib.path(), "pkg", "1.0.0");
    set_mtime(&pkg_dir.join("DESCRIPTION"), 1_000);
    let sources = tempfile::tempdir().unwrap();
    fs::write(sources.path().join("f.R"), "f <- function() 1\n").unwrap();
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    let pkg = db.package_by_name("pkg").unwrap();
    db.set_package_sources(pkg, sources.path());
    assert_eq!(pkg.sources_revision(&db), Some(revision(1_000)));

    fs::remove_dir_all(&pkg_dir).unwrap();
    rescan(&mut db, root);
    assert_eq!(pkg.sources_revision(&db), None);

    write_installed_package(lib.path(), "pkg", "1.0.0");
    set_mtime(&pkg_dir.join("DESCRIPTION"), 1_000);
    rescan(&mut db, root);

    assert_eq!(db.package_by_name("pkg"), Some(pkg));
    assert_eq!(pkg.description_revision(&db), revision(1_000));
    assert_eq!(pkg.sources_revision(&db), None);
    assert!(pkg.files(&db).is_empty());
}

#[test]
fn test_rescan_keeps_open_buffer_of_removed_package_visible() {
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
    rescan(&mut db, root);

    assert!(pkg.files(&db).is_empty());
    assert_eq!(file.package(&db), None);
    assert!(db.orphan_root().files(&db).contains(&file));
    assert_eq!(db.file_by_path(&path), Some(file));
}

#[test]
fn test_rescan_upgrade_clears_sources() {
    let lib = tempfile::tempdir().unwrap();
    let description = lib.path().join("pkg").join("DESCRIPTION");
    // `Collate` excludes `script.R`, putting it in `scripts` so the test covers
    // clearing both package source lists.
    let write_description = |version: &str| {
        fs::create_dir_all(lib.path().join("pkg")).unwrap();
        fs::write(
            &description,
            format!("Package: pkg\nVersion: {version}\nCollate: 'f.R'\n"),
        )
        .unwrap();
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
    assert!(db.file_by_path(file.path(&db)).is_none());
    assert!(db.file_by_path(script.path(&db)).is_none());

    // Refetched sources at the same paths must reuse the same `File` entities.
    db.set_package_sources(pkg, sources.path());
    assert_eq!(pkg.files(&db), &vec![file]);
    assert_eq!(pkg.scripts(&db), &vec![script]);
    assert_eq!(file.package(&db), Some(pkg));
    assert_eq!(db.file_by_path(file.path(&db)), Some(file));
}

#[test]
fn test_rescan_upgrade_keeps_open_buffer_visible() {
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
    rescan(&mut db, root);

    assert_eq!(db.package_by_name("pkg"), Some(pkg));
    assert!(pkg.files(&db).is_empty());
    assert_eq!(file.package(&db), None);
    assert!(db.orphan_root().files(&db).contains(&file));
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
    let files = pkg.files(&db).to_vec();

    // A `NAMESPACE` revision change must not discard sources when the
    // `DESCRIPTION` revision is unchanged.
    set_mtime(&pkg_dir.join("NAMESPACE"), 2_000);
    rescan(&mut db, root);

    assert_eq!(pkg.namespace_revision(&db), revision(2_000));
    assert_eq!(pkg.files(&db), files.as_slice());
    assert_eq!(files[0].package(&db), Some(pkg));
    assert_eq!(db.file_by_path(files[0].path(&db)), Some(files[0]));
}

fn primitive_stubs() -> tempfile::TempDir {
    let stubs = tempfile::tempdir().unwrap();
    fs::write(
        stubs.path().join("primitives.R"),
        "sum <- function(...) NULL\n",
    )
    .unwrap();
    stubs
}

fn library_with_stubbed_base(
    stubs: &Path,
    sources: Option<&Path>,
) -> (tempfile::TempDir, OakDatabase, Root, Package) {
    let lib = tempfile::tempdir().unwrap();
    write_installed_package(lib.path(), "base", "4.5.0");
    set_mtime(&lib.path().join("base").join("DESCRIPTION"), 1_000);
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let root = library_root(&db, lib.path());
    let pkg = db.package_by_name("base").unwrap();
    if let Some(sources) = sources {
        db.set_package_sources(pkg, sources);
    }
    db.set_package_generated_files(pkg, stubs);
    (lib, db, root, pkg)
}

#[test]
fn test_unchanged_rescan_keeps_generated_files_of_unfetched_package() {
    let stubs = primitive_stubs();
    let (_lib, mut db, root, pkg) = library_with_stubbed_base(stubs.path(), None);
    let stub = pkg.files(&db)[0];
    assert_eq!(pkg.sources_revision(&db), None);

    let before = salsa::plumbing::current_revision(&db);
    rescan(&mut db, root);

    assert_eq!(salsa::plumbing::current_revision(&db), before);
    assert_eq!(pkg.files(&db), &vec![stub]);
    assert_eq!(stub.package(&db), Some(pkg));
}

#[test]
fn test_rescan_upgrade_clears_sources_and_keeps_generated_files() {
    let stubs = primitive_stubs();
    let sources = tempfile::tempdir().unwrap();
    fs::write(sources.path().join("f.R"), "f <- function() 1\n").unwrap();
    let (lib, mut db, root, pkg) = library_with_stubbed_base(stubs.path(), Some(sources.path()));
    let [file, stub] = pkg.files(&db)[..] else {
        panic!("expected one source file and one stub");
    };
    assert_eq!(pkg.sources_revision(&db), Some(revision(1_000)));

    set_mtime(&lib.path().join("base").join("DESCRIPTION"), 2_000);
    rescan(&mut db, root);

    assert_eq!(pkg.sources_revision(&db), None);
    assert_eq!(pkg.files(&db), &vec![stub]);
    assert_eq!(file.package(&db), None);
    assert_eq!(stub.package(&db), Some(pkg));
}

#[test]
fn test_rescan_removal_clears_generated_files() {
    let stubs = primitive_stubs();
    let sources = tempfile::tempdir().unwrap();
    fs::write(sources.path().join("f.R"), "f <- function() 1\n").unwrap();
    let (lib, mut db, root, pkg) = library_with_stubbed_base(stubs.path(), Some(sources.path()));
    let stub = pkg.files(&db)[1];

    fs::remove_dir_all(lib.path().join("base")).unwrap();
    rescan(&mut db, root);

    assert_eq!(pkg.sources_revision(&db), None);
    assert!(pkg.files(&db).is_empty());
    assert_eq!(stub.package(&db), None);
}

#[test]
fn test_rescan_upgrade_keeps_files_of_package_shared_with_workspace() {
    // The workspace scan reuses the library's `Package` because both scans
    // find the same `DESCRIPTION` path.
    let workspace_dir = tempfile::tempdir().unwrap();
    let lib = workspace_dir.path().join("lib");
    let pkg_dir = lib.join("pkg");
    write_installed_package(&lib, "pkg", "1.0.0");
    fs::create_dir_all(pkg_dir.join("R")).unwrap();
    fs::write(pkg_dir.join("R").join("a.R"), "a <- 1\n").unwrap();
    set_mtime(&pkg_dir.join("DESCRIPTION"), 1_000);
    let mut db = OakDatabase::new();
    db.set_library_paths(slice::from_ref(&lib));
    let library = library_root(&db, &lib);
    let mut scheduler = ScanScheduler::new();
    let requests = scheduler.set_workspace_paths(&mut db, &[workspace_dir.path().to_path_buf()]);
    drain_scheduler(&mut db, &mut scheduler, requests);
    let workspace = db.workspace_roots().roots(&db)[0];
    let pkg = library.packages(&db)[0];
    assert_eq!(workspace.packages(&db), &vec![pkg]);
    let files = pkg.files(&db).to_vec();
    assert_eq!(files.len(), 1);

    write_installed_package(&lib, "pkg", "2.0.0");
    set_mtime(&pkg_dir.join("DESCRIPTION"), 2_000);
    rescan(&mut db, library);

    assert_eq!(pkg.description_revision(&db), revision(2_000));
    assert_eq!(pkg.files(&db), files.as_slice());
    assert_eq!(files[0].package(&db), Some(pkg));
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
