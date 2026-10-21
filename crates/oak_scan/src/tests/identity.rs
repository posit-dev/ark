//! Identity and editor-override tests. [`oak_db::IdentityMap`] returns the
//! same entity for a path independently of membership. Editor overrides exist
//! only while their buffers are open.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::slice;

use aether_path::FilePath;
use oak_db::DbInputs;
use oak_db::FileRevision;
use oak_db::OakDatabase;
use oak_db::SourceDb;
use oak_package_metadata::tests::mark_installed;
use salsa::Setter;

use crate::inputs::FileEntry;
use crate::inputs::FileUpserts;
use crate::packages::file_revision;
use crate::scheduler::drain_scheduler;
use crate::DbScan;
use crate::ScanScheduler;

fn write_package(dir: &Path, name: &str, r_files: &[(&str, &str)]) {
    fs::create_dir_all(dir.join("R")).unwrap();
    fs::write(
        dir.join("DESCRIPTION"),
        format!("Package: {name}\nVersion: 0.0.0\n"),
    )
    .unwrap();
    for (basename, contents) in r_files {
        fs::write(dir.join("R").join(basename), contents).unwrap();
    }
}

fn file_path(path: &Path) -> FilePath {
    FilePath::from_path_buf(path.to_path_buf()).unwrap()
}

fn set_workspace_paths(db: &mut OakDatabase, scheduler: &mut ScanScheduler, paths: &[PathBuf]) {
    let requests = scheduler.set_workspace_paths(db, paths);
    drain_scheduler(db, scheduler, requests);
}

#[test]
fn test_held_handle_stays_current() {
    // The handle retained by an open buffer must see every `upsert_editor()`
    // update, including across rescans, eviction, and overlapping roots.
    let tmp = tempfile::tempdir().unwrap();
    let outer = tmp.path().join("outer");
    let inner = outer.join("inner");
    write_package(&inner.join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let disk = inner.join("pkg/R/a.R");
    let path = file_path(&disk);
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    set_workspace_paths(&mut db, &mut scheduler, slice::from_ref(&inner));
    let held = db.upsert_editor(path.clone(), "v0\n".to_string());

    fs::remove_file(&disk).unwrap();
    let rprofile = inner.join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let requests = scheduler.apply_watcher_events(&mut db, vec![file_path(&rprofile)]);
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests);
    // Each step asserts visibility before `upsert_editor()`, which restores
    // it by itself and would hide a missing rescue.
    assert_eq!(db.file_by_path(&path), Some(held));
    assert_eq!(db.upsert_editor(path.clone(), "v1\n".to_string()), held);
    fs::write(&disk, "x <- 1\n").unwrap();

    set_workspace_paths(&mut db, &mut scheduler, &[]);
    assert_eq!(db.file_by_path(&path), Some(held));
    assert_eq!(db.upsert_editor(path.clone(), "v2\n".to_string()), held);

    set_workspace_paths(&mut db, &mut scheduler, slice::from_ref(&inner));
    assert_eq!(db.file_by_path(&path), Some(held));
    assert_eq!(db.upsert_editor(path.clone(), "v3\n".to_string()), held);

    set_workspace_paths(&mut db, &mut scheduler, &[outer.clone(), inner.clone()]);
    set_workspace_paths(&mut db, &mut scheduler, &[inner]);
    // Eviction of `outer` orphaned the open file, and the survivor's repair
    // rescan must take it back out and restore its package.
    assert_eq!(db.file_by_path(&path), Some(held));
    assert!(!db.orphan_root().files(&db).contains(&held));
    let pkg = held.package(&db).unwrap();
    assert_eq!(pkg.files(&db), &vec![held]);
    assert_eq!(db.upsert_editor(path.clone(), "v4\n".to_string()), held);

    assert_eq!(db.file_by_path(&path), Some(held));
    assert_eq!(held.source_text(&db), "v4\n");
}

#[test]
fn test_open_interned_unpublished_file() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let path = file_path(&tmp.path().join("pkg/R/a.R"));
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    let workspace = [tmp.path().to_path_buf()];
    set_workspace_paths(&mut db, &mut scheduler, &workspace);
    let scanned = db.file_by_path(&path).unwrap();
    set_workspace_paths(&mut db, &mut scheduler, &[]);
    assert_eq!(db.file_by_path(&path), None);

    let opened = db.upsert_editor(path.clone(), "edited\n".to_string());

    assert_eq!(opened, scanned);
    assert_eq!(db.file_by_path(&path), Some(scanned));
    assert!(db.orphan_root().files(&db).contains(&scanned));
    assert_eq!(scanned.package(&db), None);
    assert_eq!(
        scanned.source_text_override(&db).as_deref(),
        Some("edited\n")
    );
}

#[test]
fn test_rescan_dropping_file_clears_its_package() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let disk = tmp.path().join("pkg/R/a.R");
    let path = file_path(&disk);
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    let workspace = [tmp.path().to_path_buf()];
    set_workspace_paths(&mut db, &mut scheduler, &workspace);
    let scanned = db.file_by_path(&path).unwrap();
    let pkg = scanned.package(&db).unwrap();

    fs::remove_file(&disk).unwrap();
    let description = file_path(&tmp.path().join("pkg/DESCRIPTION"));
    let requests = scheduler.apply_watcher_events(&mut db, vec![description]);
    drain_scheduler(&mut db, &mut scheduler, requests);
    assert!(pkg.files(&db).is_empty());
    assert_eq!(db.file_by_path(&path), None);
    assert_eq!(scanned.package(&db), None);

    let opened = db.upsert_editor(path.clone(), "edited\n".to_string());

    assert_eq!(opened, scanned);
    assert_eq!(db.file_by_path(&path), Some(scanned));
    assert!(db.orphan_root().files(&db).contains(&scanned));
}

#[test]
fn test_open_file_of_dropped_package_clears_its_package() {
    // Opening a file from a package omitted by a rescan must clear its
    // package backpointer for placement in `OrphanRoot`. The unpublished
    // package still lists its files, whose backpointers remain intact.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let path = file_path(&tmp.path().join("pkg/R/a.R"));
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    let workspace = [tmp.path().to_path_buf()];
    set_workspace_paths(&mut db, &mut scheduler, &workspace);
    let scanned = db.file_by_path(&path).unwrap();
    let pkg = scanned.package(&db).unwrap();

    fs::remove_dir_all(tmp.path().join("pkg")).unwrap();
    let description = file_path(&tmp.path().join("pkg/DESCRIPTION"));
    let requests = scheduler.apply_watcher_events(&mut db, vec![description]);
    drain_scheduler(&mut db, &mut scheduler, requests);
    assert!(db.workspace_roots().roots(&db)[0].packages(&db).is_empty());
    assert_eq!(db.file_by_path(&path), None);
    assert_eq!(scanned.package(&db), Some(pkg));

    let opened = db.upsert_editor(path.clone(), "edited\n".to_string());

    assert_eq!(opened, scanned);
    assert_eq!(db.file_by_path(&path), Some(scanned));
    assert!(db.orphan_root().files(&db).contains(&scanned));
    assert_eq!(scanned.package(&db), None);
}

#[test]
fn test_close_editor_clears_override_of_unpublished_entity() {
    // `close_editor()` must find the override through the identity map even
    // when `file_by_path()` cannot see the entity.
    let tmp = tempfile::tempdir().unwrap();
    let disk = tmp.path().join("a.R");
    fs::write(&disk, "disk\n").unwrap();
    let path = file_path(&disk);
    let mut db = OakDatabase::new();

    let file = db.intern_file(path.clone(), FileRevision::zero(), None);
    file.set_source_text_override(&mut db)
        .to(Some("buffer\n".to_string()));
    assert_eq!(db.file_by_path(&path), None);

    db.close_editor(&path);

    assert_eq!(file.source_text_override(&db), &None);
    assert_eq!(file.source_text(&db), "disk\n");
}

#[test]
fn test_close_editor_in_container_reads_disk_and_keeps_placement() {
    let tmp = tempfile::tempdir().unwrap();
    let disk = tmp.path().join("a.R");
    fs::write(&disk, "disk\n").unwrap();
    let path = file_path(&disk);
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    let workspace = [tmp.path().to_path_buf()];
    set_workspace_paths(&mut db, &mut scheduler, &workspace);
    let root = db.workspace_roots().roots(&db)[0];
    let file = db.upsert_editor(path.clone(), "buffer\n".to_string());
    assert_eq!(file.source_text(&db), "buffer\n");

    db.close_editor(&path);

    assert_eq!(file.source_text_override(&db), &None);
    assert_eq!(file.source_text(&db), "disk\n");
    assert_eq!(root.scripts(&db).clone(), vec![file]);
    assert!(!db.orphan_root().files(&db).contains(&file));
}

#[test]
fn test_close_editor_in_orphan_reads_disk_and_unpublishes() {
    let tmp = tempfile::tempdir().unwrap();
    let disk = tmp.path().join("a.R");
    fs::write(&disk, "disk\n").unwrap();
    let path = file_path(&disk);
    let mut db = OakDatabase::new();

    let file = db.upsert_editor(path.clone(), "buffer\n".to_string());
    assert!(db.orphan_root().files(&db).contains(&file));

    db.close_editor(&path);

    assert_eq!(file.source_text_override(&db), &None);
    assert_eq!(file.source_text(&db), "disk\n");
    assert!(!db.orphan_root().files(&db).contains(&file));
    assert_eq!(db.file_by_path(&path), None);
    assert_eq!(db.interned_file(&path), Some(file));

    let reopened = db.upsert_editor(path.clone(), "again\n".to_string());
    assert_eq!(reopened, file);
    assert!(db.orphan_root().files(&db).contains(&file));
    assert_eq!(file.source_text(&db), "again\n");
}

#[test]
fn test_closed_orphan_buffer_is_not_rescued_by_later_writes() {
    // Later placement updates must not rescue a closed buffer back into
    // `OrphanRoot`.
    let tmp = tempfile::tempdir().unwrap();
    let path = file_path(&tmp.path().join("a.R"));
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    let file = db.upsert_editor(path.clone(), "buffer\n".to_string());
    db.close_editor(&path);
    assert_eq!(db.file_by_path(&path), None);

    db.set_library_paths(&[]);
    set_workspace_paths(&mut db, &mut scheduler, &[]);

    assert_eq!(db.file_by_path(&path), None);
    assert!(!db.orphan_root().files(&db).contains(&file));
}

#[test]
fn test_close_editor_unknown_path_creates_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let path = file_path(&tmp.path().join("never-opened.R"));
    let mut db = OakDatabase::new();

    db.close_editor(&path);

    assert_eq!(db.interned_file(&path), None);
}

#[test]
fn test_close_editor_twice_is_revision_neutral() {
    let tmp = tempfile::tempdir().unwrap();
    let path = file_path(&tmp.path().join("a.R"));
    let mut db = OakDatabase::new();

    db.upsert_editor(path.clone(), "buffer\n".to_string());
    db.close_editor(&path);
    let before = salsa::plumbing::current_revision(&db);

    db.close_editor(&path);

    assert_eq!(salsa::plumbing::current_revision(&db), before);
}

#[test]
fn test_library_root_readd_refreshes_index_revision() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    write_package(&lib.join("pkg"), "pkg", &[]);
    mark_installed(&lib.join("pkg"));
    let mut db = OakDatabase::new();

    db.set_library_paths(slice::from_ref(&lib));
    let pkg = db.library_roots().roots(&db)[0].packages(&db)[0];

    db.set_library_paths(&[]);
    let index = lib.join("pkg/INDEX");
    fs::write(&index, "data    Some data\n").unwrap();
    db.set_library_paths(&[lib]);

    assert_eq!(
        db.library_roots().roots(&db)[0].packages(&db).clone(),
        vec![pkg]
    );
    assert_eq!(pkg.index_revision(&db), Some(file_revision(&index)));
}

#[test]
fn test_library_root_readd_reuses_packages_and_files() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    fs::create_dir_all(lib.join("pkg")).unwrap();
    fs::write(
        lib.join("pkg/DESCRIPTION"),
        "Package: pkg\nVersion: 1.0.0\n",
    )
    .unwrap();
    mark_installed(&lib.join("pkg"));
    let sources = tmp.path().join("sources");
    fs::create_dir_all(&sources).unwrap();
    fs::write(sources.join("a.R"), "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();

    db.set_library_paths(slice::from_ref(&lib));
    let pkg = db.library_roots().roots(&db)[0].packages(&db)[0];
    db.set_package_sources(pkg, &sources);
    let files = pkg.files(&db).to_vec();
    assert_eq!(files.len(), 1);

    db.set_library_paths(&[]);
    assert!(pkg.files(&db).is_empty());

    db.set_library_paths(&[lib]);
    assert_eq!(
        db.library_roots().roots(&db)[0].packages(&db).clone(),
        vec![pkg]
    );
    db.set_package_sources(pkg, &sources);
    assert_eq!(pkg.files(&db), files.as_slice());
}

#[test]
fn test_workspace_root_readd_reuses_entities_and_refreshes_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let path = file_path(&tmp.path().join("pkg/R/a.R"));
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    let workspace = [tmp.path().to_path_buf()];
    set_workspace_paths(&mut db, &mut scheduler, &workspace);
    let pkg = db.workspace_roots().roots(&db)[0].packages(&db)[0];
    let file = db.file_by_path(&path).unwrap();

    set_workspace_paths(&mut db, &mut scheduler, &[]);
    let description = tmp.path().join("pkg/DESCRIPTION");
    fs::write(&description, "Package: renamed\nVersion: 0.0.1\n").unwrap();
    set_workspace_paths(&mut db, &mut scheduler, &workspace);

    assert_eq!(
        db.workspace_roots().roots(&db)[0].packages(&db).clone(),
        vec![pkg]
    );
    assert_eq!(pkg.name(&db), "renamed");
    assert_eq!(pkg.description_revision(&db), file_revision(&description));
    assert_eq!(pkg.files(&db), &[file]);
    assert_eq!(file.package(&db), Some(pkg));
}

// Documents current behavior: a workspace package and a library package at the
// same `DESCRIPTION` share one `Package`. Whether they should stays undecided.
// Libraries list only installed packages and the workspace walk prunes them, so
// both roots hold the package only while a directory changes between source
// and installed and one of them hasn't rescanned yet.
#[test]
fn test_library_registration_shares_workspace_package() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    write_package(&lib.join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    set_workspace_paths(&mut db, &mut scheduler, slice::from_ref(&lib));
    let workspace_pkg = db.workspace_roots().roots(&db)[0].packages(&db)[0];

    mark_installed(&lib.join("pkg"));
    db.set_library_paths(&[lib]);

    assert_eq!(
        db.library_roots().roots(&db)[0].packages(&db).clone(),
        vec![workspace_pkg]
    );
}

#[test]
#[should_panic(expected = "`FileUpserts` dropped without `finish()`")]
fn test_file_upserts_dropped_without_finish_panics_in_debug() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let mut upserts = FileUpserts::new();
    upserts.upsert(&mut db, None, FileEntry {
        path: file_path(&tmp.path().join("a.R")),
        revision: FileRevision::zero(),
    });
}
