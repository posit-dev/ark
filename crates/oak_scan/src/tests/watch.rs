use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::DbInputs;
use oak_db::OakDatabase;
use oak_db::Root;

use crate::scheduler::drain_scheduler;
use crate::DbScan;
use crate::FileEvent;
use crate::FileEventKind;
use crate::ScanRequest;
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

/// Sync helper: set workspace paths and drive the scheduler to
/// quiescence on the current thread.
fn set_workspace_paths(db: &mut OakDatabase, paths: &[PathBuf], editor_owned: &HashSet<FilePath>) {
    let mut scheduler = ScanScheduler::new();
    let reqs = scheduler.set_workspace_paths(db, paths, editor_owned);
    drain_scheduler(db, &mut scheduler, reqs, editor_owned);
}

/// Sync helper: dispatch watcher events through the scheduler and
/// drive it to quiescence.
fn apply_watcher_events(db: &mut OakDatabase, events: Vec<FileEvent>, skip: &HashSet<FilePath>) {
    let mut scheduler = ScanScheduler::new();
    let reqs = scheduler.apply_watcher_events(db, events, skip);
    drain_scheduler(db, &mut scheduler, reqs, skip);
}

/// Sync helper: synthesize a single Changed watcher event. The
/// scheduler reads contents from disk, so callers must write the
/// expected content to disk before calling.
fn add_watched_file(db: &mut OakDatabase, path: FilePath) {
    apply_watcher_events(
        db,
        vec![FileEvent {
            kind: FileEventKind::Changed,
            path,
        }],
        &HashSet::new(),
    );
}

fn remove_watched_file(db: &mut OakDatabase, path: FilePath) {
    apply_watcher_events(
        db,
        vec![FileEvent {
            kind: FileEventKind::Deleted,
            path,
        }],
        &HashSet::new(),
    );
}

/// Sync helper: force a fresh full rescan of `root`. Equivalent to the
/// production trigger of a DESCRIPTION watcher event hitting the root.
fn rescan_workspace_root(db: &mut OakDatabase, root: Root) {
    let path = root.path(db).as_path().unwrap().to_path_buf();
    let result = ScanRequest { root, path }.run();
    let mut scheduler = ScanScheduler::new();
    let reqs = scheduler.apply_scan_completed(db, result, &HashSet::new());
    drain_scheduler(db, &mut scheduler, reqs, &HashSet::new());
}

#[test]
fn test_add_watched_file_new_top_level_script() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let path = tmp.path().join("new.R");
    fs::write(&path, "x <- 1\n").unwrap();
    let path = FilePath::from_path_buf(path.clone()).unwrap();
    add_watched_file(&mut db, path.clone());

    let scripts = db.workspace_roots().roots(&db)[0].scripts(&db).clone();
    assert_eq!(scripts.len(), 1);
    assert!(scripts[0].path(&db).to_url().path().ends_with("new.R"));
}

#[test]
fn test_add_watched_file_into_existing_package() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let path = tmp.path().join("pkg/R/b.R");
    fs::write(&path, "y <- 2\n").unwrap();
    let path = FilePath::from_path_buf(path.clone()).unwrap();
    add_watched_file(&mut db, path.clone());

    let packages = db.workspace_roots().roots(&db)[0].packages(&db).clone();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].files(&db).len(), 2);

    let file = db.file_by_path(&path).expect("findable");
    assert_eq!(file.package(&db), Some(packages[0]));
}

#[test]
fn test_add_watched_file_routes_package_subdir_to_pkg_scripts() {
    // R files inside `<pkg>/tests/` and similar non-`R/` subdirs go to
    // `pkg.scripts`, matching the bulk scanner. They're not workspace
    // scripts and not part of `pkg.files`.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    fs::create_dir_all(tmp.path().join("pkg/tests")).unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let path = tmp.path().join("pkg/tests/test-foo.R");
    fs::write(&path, "test code\n").unwrap();
    let path = FilePath::from_path_buf(path.clone()).unwrap();
    add_watched_file(&mut db, path.clone());

    let root = db.workspace_roots().roots(&db)[0];
    let pkg = root.packages(&db)[0];
    assert!(root.scripts(&db).is_empty());
    assert_eq!(pkg.files(&db).len(), 1);
    assert_eq!(pkg.scripts(&db).len(), 1);
    let file = db.file_by_path(&path).expect("findable via pkg.scripts");
    assert_eq!(file.package(&db), Some(pkg));
    assert_eq!(file.source_text(&db), "test code\n");
}

#[test]
fn test_add_watched_file_skips_nested_r_subdir() {
    // `<pkg>/R/` is flat in standard R packages. The bulk scanner ignores
    // anything deeper than `<pkg>/R/*.R`, so the watcher does too instead
    // of placing it in `pkg.scripts` where the next rescan would drop it.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    fs::create_dir_all(tmp.path().join("pkg/R/nested")).unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let path = tmp.path().join("pkg/R/nested/deep.R");
    fs::write(&path, "z <- 3\n").unwrap();
    let path = FilePath::from_path_buf(path.clone()).unwrap();
    add_watched_file(&mut db, path.clone());

    let root = db.workspace_roots().roots(&db)[0];
    let pkg = root.packages(&db)[0];
    assert_eq!(pkg.files(&db).len(), 1);
    assert!(pkg.scripts(&db).is_empty());
    assert!(db.file_by_path(&path).is_none());
}

#[test]
fn test_add_watched_file_updates_pkg_scripts_content_preserves_placement() {
    // Edit of an existing `pkg.scripts` file: contents change, the file
    // stays in `pkg.scripts` (no duplicate, no move to `pkg.files`).
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    fs::create_dir_all(tmp.path().join("pkg/tests")).unwrap();
    fs::write(tmp.path().join("pkg/tests/test-foo.R"), "v1\n").unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let fs_path = tmp.path().join("pkg/tests/test-foo.R");
    let path = FilePath::from_path_buf(fs_path.clone()).unwrap();
    let pkg = db.workspace_roots().roots(&db)[0].packages(&db)[0];
    let file_before = pkg.scripts(&db)[0];

    fs::write(&fs_path, "v2\n").unwrap();
    add_watched_file(&mut db, path.clone());

    let file_after = db.file_by_path(&path).unwrap();
    assert_eq!(file_before, file_after);
    assert_eq!(file_after.source_text(&db), "v2\n");
    assert_eq!(file_after.package(&db), Some(pkg));
    assert_eq!(pkg.scripts(&db).len(), 1);
    assert_eq!(pkg.files(&db).len(), 1);
}

#[test]
fn test_remove_watched_file_from_pkg_scripts() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    fs::create_dir_all(tmp.path().join("pkg/tests")).unwrap();
    fs::write(tmp.path().join("pkg/tests/test-foo.R"), "t\n").unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let path = tmp.path().join("pkg/tests/test-foo.R");
    let path = FilePath::from_path_buf(path.clone()).unwrap();
    remove_watched_file(&mut db, path.clone());

    let pkg = db.workspace_roots().roots(&db)[0].packages(&db)[0];
    assert!(pkg.scripts(&db).is_empty());
    assert_eq!(pkg.files(&db).len(), 1);
    assert!(db.file_by_path(&path).is_none());
}

#[test]
fn test_add_watched_file_outside_workspace_is_skipped() {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(
        &mut db,
        &[workspace.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    let path = outside.path().join("stray.R");
    fs::write(&path, "z <- 3\n").unwrap();
    let path = FilePath::from_path_buf(path.clone()).unwrap();
    add_watched_file(&mut db, path.clone());

    assert!(db.file_by_path(&path).is_none());
    let root = db.workspace_roots().roots(&db)[0];
    assert!(root.scripts(&db).is_empty());
}

#[test]
fn test_add_watched_file_updates_existing_content_preserves_placement() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "v1\n")]);
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let fs_path = tmp.path().join("pkg/R/a.R");
    let path = FilePath::from_path_buf(fs_path.clone()).unwrap();
    let pkg = db.workspace_roots().roots(&db)[0].packages(&db)[0];
    let file_before = pkg.files(&db)[0];

    fs::write(&fs_path, "v2\n").unwrap();
    add_watched_file(&mut db, path.clone());

    let file_after = db.file_by_path(&path).unwrap();
    assert_eq!(file_before, file_after);
    assert_eq!(file_after.source_text(&db), "v2\n");
    assert_eq!(file_after.package(&db), Some(pkg));
    assert_eq!(pkg.files(&db).len(), 1);
}

#[test]
fn test_remove_watched_file_from_package() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[
        ("a.R", "x <- 1\n"),
        ("b.R", "y <- 2\n"),
    ]);
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let path = tmp.path().join("pkg/R/a.R");
    let path = FilePath::from_path_buf(path.clone()).unwrap();
    remove_watched_file(&mut db, path.clone());

    let pkg = db.workspace_roots().roots(&db)[0].packages(&db)[0];
    assert_eq!(pkg.files(&db).len(), 1);
    assert!(db.file_by_path(&path).is_none());
}

#[test]
fn test_remove_watched_file_from_workspace_scripts() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.R"), "x <- 1\n").unwrap();
    fs::write(tmp.path().join("b.R"), "y <- 2\n").unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let path = tmp.path().join("a.R");
    let path = FilePath::from_path_buf(path.clone()).unwrap();
    remove_watched_file(&mut db, path.clone());

    let scripts = db.workspace_roots().roots(&db)[0].scripts(&db).clone();
    assert_eq!(scripts.len(), 1);
    assert!(db.file_by_path(&path).is_none());
}

#[test]
fn test_remove_watched_file_unknown_url_is_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let path = FilePath::from_path_buf(tmp.path().join("ghost.R")).unwrap();
    remove_watched_file(&mut db, path);
}

#[test]
fn test_rescan_workspace_root_picks_up_new_description() {
    // A `DESCRIPTION` appears after the initial scan: a former script
    // directory is now a package. Surgical add_watched_file can't handle this,
    // so the LSP layer falls back to a root rescan.
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("pkg/R")).unwrap();
    fs::write(tmp.path().join("pkg/R/a.R"), "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    // No DESCRIPTION yet, so the R file came in as a script.
    let root = db.workspace_roots().roots(&db)[0];
    assert!(root.packages(&db).is_empty());
    assert_eq!(root.scripts(&db).len(), 1);

    fs::write(
        tmp.path().join("pkg/DESCRIPTION"),
        "Package: pkg\nVersion: 0.0.0\n",
    )
    .unwrap();
    rescan_workspace_root(&mut db, root);

    assert_eq!(root.packages(&db).len(), 1);
    assert_eq!(root.packages(&db)[0].files(&db).len(), 1);
    assert!(root.scripts(&db).is_empty());
}

#[test]
fn test_rescan_workspace_root_drops_removed_pkg_scripts() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    fs::create_dir_all(tmp.path().join("pkg/tests")).unwrap();
    fs::write(tmp.path().join("pkg/tests/test-foo.R"), "t\n").unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let root = db.workspace_roots().roots(&db)[0];
    let pkg = root.packages(&db)[0];
    assert_eq!(pkg.scripts(&db).len(), 1);

    fs::remove_file(tmp.path().join("pkg/tests/test-foo.R")).unwrap();
    rescan_workspace_root(&mut db, root);

    assert!(pkg.scripts(&db).is_empty());
    assert_eq!(pkg.files(&db).len(), 1);
}

#[test]
fn test_rescan_workspace_root_preserves_pkg_scripts_identity() {
    // A rescan with no on-disk changes should reuse the same `File`
    // entity for a file already in `pkg.scripts`. Identity matters for
    // downstream salsa caches keyed on `File`.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    fs::create_dir_all(tmp.path().join("pkg/tests")).unwrap();
    fs::write(tmp.path().join("pkg/tests/test-foo.R"), "t\n").unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let root = db.workspace_roots().roots(&db)[0];
    let pkg = root.packages(&db)[0];
    let file_before = pkg.scripts(&db)[0];

    rescan_workspace_root(&mut db, root);

    let pkg = db.workspace_roots().roots(&db)[0].packages(&db)[0];
    assert_eq!(pkg.scripts(&db).len(), 1);
    assert_eq!(pkg.scripts(&db)[0], file_before);
}

#[test]
fn test_rescan_workspace_root_demotes_removed_description() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let root = db.workspace_roots().roots(&db)[0];
    assert_eq!(root.packages(&db).len(), 1);

    fs::remove_file(tmp.path().join("pkg/DESCRIPTION")).unwrap();
    rescan_workspace_root(&mut db, root);

    assert!(root.packages(&db).is_empty());
    // The R file under pkg/R/ is no longer in a recognised package, so
    // it surfaces as a workspace script.
    assert_eq!(root.scripts(&db).len(), 1);
}

fn file_event(path: &Path, kind: FileEventKind) -> FileEvent {
    FileEvent {
        path: FilePath::from_path_buf(path.to_path_buf()).unwrap(),
        kind,
    }
}

#[test]
fn test_apply_watcher_events_routes_description_to_rescan() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("pkg/R")).unwrap();
    fs::write(tmp.path().join("pkg/R/a.R"), "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    fs::write(
        tmp.path().join("pkg/DESCRIPTION"),
        "Package: pkg\nVersion: 0.0.0\n",
    )
    .unwrap();
    apply_watcher_events(
        &mut db,
        vec![file_event(
            &tmp.path().join("pkg/DESCRIPTION"),
            FileEventKind::Created,
        )],
        &HashSet::new(),
    );

    let root = db.workspace_roots().roots(&db)[0];
    assert_eq!(root.packages(&db).len(), 1);
}

#[test]
fn test_apply_watcher_events_dedupes_descriptions_per_root() {
    // Two DESCRIPTION events under the same root: rescan_workspace_root
    // should fire once. We can't observe the call count directly, but
    // we can check the final state is correct.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg1"), "pkg1", &[]);
    write_package(&tmp.path().join("pkg2"), "pkg2", &[]);
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    apply_watcher_events(
        &mut db,
        vec![
            file_event(&tmp.path().join("pkg1/DESCRIPTION"), FileEventKind::Changed),
            file_event(&tmp.path().join("pkg2/DESCRIPTION"), FileEventKind::Changed),
        ],
        &HashSet::new(),
    );

    let root = db.workspace_roots().roots(&db)[0];
    assert_eq!(root.packages(&db).len(), 2);
}

#[test]
fn test_apply_watcher_events_routes_r_file_to_add() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let path = tmp.path().join("new.R");
    fs::write(&path, "x <- 1\n").unwrap();
    apply_watcher_events(
        &mut db,
        vec![file_event(&path, FileEventKind::Created)],
        &HashSet::new(),
    );

    let root = db.workspace_roots().roots(&db)[0];
    assert_eq!(root.scripts(&db).len(), 1);
}

#[test]
fn test_apply_watcher_events_routes_r_file_to_remove() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("a.R"), "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let fs_path = tmp.path().join("a.R");
    let path = FilePath::from_path_buf(fs_path.clone()).unwrap();
    apply_watcher_events(
        &mut db,
        vec![file_event(&fs_path, FileEventKind::Deleted)],
        &HashSet::new(),
    );

    let root = db.workspace_roots().roots(&db)[0];
    assert!(root.scripts(&db).is_empty());
    assert!(db.file_by_path(&path).is_none());
}

#[test]
fn test_apply_watcher_events_skip_set_blocks_r_file_event() {
    let tmp = tempfile::tempdir().unwrap();
    let fs_path = tmp.path().join("a.R");
    fs::write(&fs_path, "disk_v1\n").unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    // Driver "owns" this URL (the editor has it open).
    let path = FilePath::from_path_buf(fs_path.clone()).unwrap();
    db.upsert_editor(path.clone(), "editor_v2\n".to_string());

    let mut skip = HashSet::new();
    skip.insert(path.clone());

    fs::write(&fs_path, "disk_v3\n").unwrap();
    apply_watcher_events(
        &mut db,
        vec![file_event(&fs_path, FileEventKind::Changed)],
        &skip,
    );

    let file = db.file_by_path(&path).unwrap();
    assert_eq!(file.source_text(&db), "editor_v2\n");
}

#[test]
fn test_apply_watcher_events_skip_set_does_not_block_description() {
    // DESCRIPTION classification is disk-authoritative, so the skip
    // set (an editor-owned URL set) should not hold back a
    // DESCRIPTION rescan even if the DESCRIPTION URL is in `skip`.
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("pkg/R")).unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    fs::write(
        tmp.path().join("pkg/DESCRIPTION"),
        "Package: pkg\nVersion: 0.0.0\n",
    )
    .unwrap();

    let desc_fs_path = tmp.path().join("pkg/DESCRIPTION");
    let desc_path = FilePath::from_path_buf(desc_fs_path.clone()).unwrap();
    let mut skip = HashSet::new();
    skip.insert(desc_path);

    apply_watcher_events(
        &mut db,
        vec![file_event(&desc_fs_path, FileEventKind::Created)],
        &skip,
    );

    let root = db.workspace_roots().roots(&db)[0];
    assert_eq!(root.packages(&db).len(), 1);
}

#[test]
fn test_apply_watcher_events_description_outside_any_workspace_is_noop() {
    // A DESCRIPTION event for a path outside every workspace root has
    // nowhere to land. The handler should ignore it rather than
    // rescanning some arbitrary root.
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(
        outside.path().join("DESCRIPTION"),
        "Package: stray\nVersion: 0.0.0\n",
    )
    .unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(
        &mut db,
        &[workspace.path().to_path_buf()],
        &std::collections::HashSet::new(),
    );

    apply_watcher_events(
        &mut db,
        vec![file_event(
            &outside.path().join("DESCRIPTION"),
            FileEventKind::Created,
        )],
        &HashSet::new(),
    );

    let root = db.workspace_roots().roots(&db)[0];
    assert!(root.packages(&db).is_empty());
}

#[test]
fn test_apply_watcher_events_ignores_non_r_files() {
    // The LSP registration filters to `*.{R,r}` and `DESCRIPTION`, so the
    // dispatcher shouldn't see other paths in practice. Defensive check that
    // `add_watched_file`'s classifier drops them silently rather than
    // landing them in the orphan bucket or some root container.
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    let fs_path = tmp.path().join("notes.txt");
    fs::write(&fs_path, "not R\n").unwrap();
    let path = FilePath::from_path_buf(fs_path.clone()).unwrap();
    apply_watcher_events(
        &mut db,
        vec![file_event(&fs_path, FileEventKind::Created)],
        &HashSet::new(),
    );

    assert!(db.file_by_path(&path).is_none());
    let root = db.workspace_roots().roots(&db)[0];
    assert!(root.scripts(&db).is_empty());
    assert!(db.orphan_root().files(&db).is_empty());
}

#[test]
fn test_apply_watcher_events_tolerates_non_package_description() {
    // The dispatcher triggers a rescan on any file named `DESCRIPTION`
    // without inspecting its contents. If the file isn't actually an R
    // package DESCRIPTION, the rescan tolerates that and leaves the
    // workspace unclassified rather than panicking or erroring.
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    fs::create_dir_all(tmp.path().join("not-a-pkg")).unwrap();
    fs::write(
        tmp.path().join("not-a-pkg/DESCRIPTION"),
        "Title: Some other project\nVersion: 1.0\n",
    )
    .unwrap();
    apply_watcher_events(
        &mut db,
        vec![file_event(
            &tmp.path().join("not-a-pkg/DESCRIPTION"),
            FileEventKind::Created,
        )],
        &HashSet::new(),
    );

    let root = db.workspace_roots().roots(&db)[0];
    assert!(root.packages(&db).is_empty());
}
