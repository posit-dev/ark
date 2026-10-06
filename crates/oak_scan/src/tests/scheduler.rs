//! Tests for the async-shape behavior of [`crate::ScanScheduler`].
//!
//! Unlike the workspace/watch tests (which drain the scheduler in a
//! single shot), these tests pause between stages: spawn the scan but
//! don't run it yet, fire other events in the middle, then run the
//! scan, then assert. That's the only way to exercise the buffering
//! and stale-result drop paths that exist precisely to handle work
//! arriving mid-scan.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::Path;
use std::path::PathBuf;

use aether_path::FilePath;
use oak_db::DbInputs;
use oak_db::OakDatabase;
use oak_db::Root;
use oak_db::SourceDb;

use crate::lookup::package_by_path;
use crate::scheduler::drain_scheduler;
use crate::tests::is_case_insensitive;
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

#[test]
fn test_stale_result_dropped_when_root_removed_mid_scan() {
    // Spawn a scan, remove the workspace folder before the scan
    // applies, then apply the result. The scan output should be
    // silently discarded since `result.root` is no longer in
    // `workspace_roots`.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    let mut requests =
        scheduler.set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());
    assert_eq!(requests.len(), 1);
    let req = requests.pop().unwrap();
    let dead_root = req.root;

    // User removes the folder while the scan is still in flight.
    let evict = scheduler.set_workspace_paths(&mut db, &[], &HashSet::new());
    assert!(evict.is_empty());
    assert!(!db.workspace_roots().roots(&db).contains(&dead_root));

    // Scan finally completes. Result should drop.
    let result = req.run();
    let followups = scheduler.apply_scan_completed(&mut db, result, &HashSet::new());
    assert!(followups.is_empty());

    // The package the scan would have created shouldn't surface.
    let pkg_path = FilePath::from_path_buf(tmp.path().join("pkg/DESCRIPTION")).unwrap();
    assert!(package_by_path(&db, &pkg_path).is_none());
}

#[test]
fn test_remove_then_readd_during_scan_uses_distinct_root_entities() {
    // The stale-result drop hinges on `Root` entity identity, not path.
    // After remove + re-add of the same path, the second add mints a
    // fresh `Root`; the first scan's result keys off the now-dead
    // first `Root` and gets dropped.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    let first = scheduler
        .set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new())
        .pop()
        .unwrap();
    let root_a = first.root;

    // Folder removed.
    scheduler.set_workspace_paths(&mut db, &[], &HashSet::new());

    // Folder re-added: distinct `Root` entity.
    let second = scheduler
        .set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new())
        .pop()
        .unwrap();
    let root_b = second.root;
    assert_ne!(root_a, root_b);

    // First scan's result lands on the dead `Root` and gets dropped.
    let result_a = first.run();
    let followups_a = scheduler.apply_scan_completed(&mut db, result_a, &HashSet::new());
    assert!(followups_a.is_empty());

    // Second scan applies normally.
    let result_b = second.run();
    let followups_b = scheduler.apply_scan_completed(&mut db, result_b, &HashSet::new());
    assert!(followups_b.is_empty());
    let pkg = db.workspace_roots().roots(&db)[0].packages(&db)[0];
    assert_eq!(pkg.name(&db), "pkg");
}

#[test]
fn test_watcher_event_buffered_during_scan_and_replayed() {
    // An R-file watcher event for a pending root should be buffered,
    // not lost. After the scan applies, the buffered event is replayed
    // and the new file appears in the right container.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    let request = scheduler
        .set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new())
        .pop()
        .unwrap();

    // Mid-scan: a new file appears under pkg/R/, the watcher fires.
    let new_path = tmp.path().join("pkg/R/b.R");
    fs::write(&new_path, "y <- 2\n").unwrap();
    let new_path = FilePath::from_path_buf(new_path.clone()).unwrap();
    let event_followups =
        scheduler.apply_watcher_events(&mut db, vec![new_path.clone()], &HashSet::new());
    // Event was buffered, not dispatched as a scan.
    assert!(event_followups.is_empty());
    // And not yet visible to the db: the scan that would create the
    // root's `Package` hasn't run yet.
    assert!(db.file_by_path(&new_path).is_none());

    // Scan completes. Buffered event replays automatically.
    let result = request.run();
    let followups = scheduler.apply_scan_completed(&mut db, result, &HashSet::new());
    assert!(followups.is_empty());

    // Both files are now present in pkg.files.
    let pkg = db.workspace_roots().roots(&db)[0].packages(&db)[0];
    assert_eq!(pkg.files(&db).len(), 2);
    assert!(db.file_by_path(&new_path).is_some());
}

#[test]
fn test_description_event_during_scan_queues_rescan() {
    // A DESCRIPTION event hitting a pending root should flip the root
    // to `ScanningWithRescanQueued`. When the first scan applies, a
    // fresh `ScanRequest` for the same root comes back.
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("pkg/R")).unwrap();
    fs::write(tmp.path().join("pkg/R/a.R"), "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    let request = scheduler
        .set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new())
        .pop()
        .unwrap();
    let root = request.root;

    // Mid-scan: DESCRIPTION appears, watcher fires.
    fs::write(
        tmp.path().join("pkg/DESCRIPTION"),
        "Package: pkg\nVersion: 0.0.0\n",
    )
    .unwrap();
    let desc_path = FilePath::from_path_buf(tmp.path().join("pkg/DESCRIPTION")).unwrap();
    let watcher_followups =
        scheduler.apply_watcher_events(&mut db, vec![desc_path], &HashSet::new());
    assert!(watcher_followups.is_empty());

    // First scan applies. It saw no DESCRIPTION yet (was written after
    // walk started in this test, but our fake `ScanRequest::run` will pick it
    // up). The queued rescan should still kick off.
    let result = request.run();
    let mut followups = scheduler.apply_scan_completed(&mut db, result, &HashSet::new());
    assert_eq!(followups.len(), 1);
    assert_eq!(followups[0].root, root);

    // Drive the queued rescan to completion.
    let req2 = followups.pop().unwrap();
    let result2 = req2.run();
    let final_followups = scheduler.apply_scan_completed(&mut db, result2, &HashSet::new());
    assert!(final_followups.is_empty());

    // Package is now classified.
    let root = db.workspace_roots().roots(&db)[0];
    assert_eq!(root.packages(&db).len(), 1);
}

#[test]
fn test_description_event_on_idle_root_returns_scan_request() {
    // A DESCRIPTION event on an idle root should kick off a fresh
    // scan, not silently no-op. The previous (sync) implementation
    // called rescan_workspace_root inline; the new contract returns a
    // `ScanRequest` for the caller to dispatch.
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("pkg/R")).unwrap();
    fs::write(tmp.path().join("pkg/R/a.R"), "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    // Initial scan: no DESCRIPTION yet, so root has no packages.
    let init = scheduler.set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());
    drain_scheduler(&mut db, &mut scheduler, init, &HashSet::new());
    let root = db.workspace_roots().roots(&db)[0];
    assert!(root.packages(&db).is_empty());

    // DESCRIPTION appears. Watcher fires while root is idle.
    fs::write(
        tmp.path().join("pkg/DESCRIPTION"),
        "Package: pkg\nVersion: 0.0.0\n",
    )
    .unwrap();
    let desc_path = FilePath::from_path_buf(tmp.path().join("pkg/DESCRIPTION")).unwrap();
    let followups = scheduler.apply_watcher_events(&mut db, vec![desc_path], &HashSet::new());
    assert_eq!(followups.len(), 1);
    assert_eq!(followups[0].root, root);

    drain_scheduler(&mut db, &mut scheduler, followups, &HashSet::new());
    assert_eq!(root.packages(&db).len(), 1);
}

#[test]
fn test_failed_scan_clears_state_and_drains_buffer() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let request = scheduler
        .set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new())
        .pop()
        .unwrap();

    let script = tmp.path().join("script.R");
    fs::write(&script, "x <- 1\n").unwrap();
    let script = FilePath::from_path_buf(script).unwrap();
    let requests = scheduler.apply_watcher_events(&mut db, vec![script.clone()], &HashSet::new());
    assert!(requests.is_empty());
    assert!(db.file_by_path(&script).is_none());

    let followups = scheduler.apply_scan_failed(&mut db, request.root, &HashSet::new());
    assert!(followups.is_empty());
    assert!(!scheduler.has_pending_scans());
    assert!(db.file_by_path(&script).is_some());
}

#[test]
fn test_failed_scan_keeps_previous_root_contents() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    fs::write(tmp.path().join("script.R"), "y <- 2\n").unwrap();
    fs::write(tmp.path().join(".Rprofile"), "").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];
    let packages = root.packages(&db).clone();
    let scripts = root.scripts(&db).clone();
    assert_eq!(packages.len(), 1);
    assert_eq!(scripts.len(), 1);

    let description = watched_path(&tmp.path().join("pkg/DESCRIPTION"));
    let mut requests = scheduler.apply_watcher_events(&mut db, vec![description], &HashSet::new());
    let request = requests.pop().unwrap();

    let followups = scheduler.apply_scan_failed(&mut db, request.root, &HashSet::new());
    assert!(followups.is_empty());
    assert!(!scheduler.has_pending_scans());
    assert_eq!(root.packages(&db), &packages);
    assert_eq!(root.scripts(&db), &scripts);
    assert_eq!(environment_dirs(&db, root), vec![tmp.path().to_path_buf()]);
}

#[test]
fn test_failed_scan_starts_queued_rescan() {
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let request = scheduler
        .set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new())
        .pop()
        .unwrap();
    let root = request.root;

    let description = tmp.path().join("DESCRIPTION");
    fs::write(&description, "Package: pkg\nVersion: 0.0.0\n").unwrap();
    let description = FilePath::from_path_buf(description).unwrap();
    let requests = scheduler.apply_watcher_events(&mut db, vec![description], &HashSet::new());
    assert!(requests.is_empty());

    let followups = scheduler.apply_scan_failed(&mut db, root, &HashSet::new());
    assert_eq!(followups.len(), 1);
    assert_eq!(followups[0].root, root);
    drain_scheduler(&mut db, &mut scheduler, followups, &HashSet::new());
    assert_eq!(root.packages(&db).len(), 1);
}

// --- Environment directories ---

fn watched_path(path: &Path) -> FilePath {
    FilePath::from_path_buf(path.to_path_buf()).unwrap()
}

fn environment_dirs(db: &OakDatabase, root: Root) -> Vec<PathBuf> {
    root.environment_dirs(db)
        .iter()
        .map(|dir| dir.as_path().unwrap().as_std_path().to_path_buf())
        .collect()
}

/// Returned roots follow `paths` order, not scan completion order.
fn scan_workspace(
    db: &mut OakDatabase,
    scheduler: &mut ScanScheduler,
    paths: &[PathBuf],
) -> Vec<Root> {
    let requests = scheduler.set_workspace_paths(db, paths, &HashSet::new());
    drain_scheduler(db, scheduler, requests, &HashSet::new());
    db.workspace_roots().roots(db).clone()
}

#[test]
fn test_scan_collects_environment_dirs() {
    // Both sentinels count, at any depth including the root itself. Hidden
    // and ignored directories aren't walked, so their sentinels don't count.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    for sub in ["a", "b/c", ".hidden", "ignored", "plain"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(dir.join(".Rprofile"), "").unwrap();
    fs::write(dir.join("a/.Rprofile"), "").unwrap();
    fs::write(dir.join("b/c/.Renviron"), "").unwrap();
    fs::write(dir.join(".hidden/.Rprofile"), "").unwrap();
    fs::write(dir.join("ignored/.Rprofile"), "").unwrap();
    fs::write(dir.join(".ignore"), "ignored/\n").unwrap();

    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let roots = scan_workspace(&mut db, &mut scheduler, &[dir.to_path_buf()]);

    assert_eq!(environment_dirs(&db, roots[0]), vec![
        dir.to_path_buf(),
        dir.join("a"),
        dir.join("b/c"),
    ]);
}

#[test]
fn test_sentinel_events_on_idle_root_rescan() {
    let tmp = tempfile::tempdir().unwrap();
    let sub = tmp.path().join("sub");
    fs::create_dir_all(&sub).unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];
    assert!(environment_dirs(&db, root).is_empty());

    let rprofile = sub.join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&rprofile)], &HashSet::new());
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());
    assert_eq!(environment_dirs(&db, root), vec![sub.clone()]);

    fs::remove_file(&rprofile).unwrap();
    let requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&rprofile)], &HashSet::new());
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());
    assert!(environment_dirs(&db, root).is_empty());
}

#[test]
fn test_deleting_one_of_two_sentinels_keeps_environment_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::write(dir.join(".Rprofile"), "").unwrap();
    fs::write(dir.join(".Renviron"), "").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[dir.to_path_buf()])[0];
    assert_eq!(environment_dirs(&db, root), vec![dir.to_path_buf()]);

    fs::remove_file(dir.join(".Rprofile")).unwrap();
    let requests = scheduler.apply_watcher_events(
        &mut db,
        vec![watched_path(&dir.join(".Rprofile"))],
        &HashSet::new(),
    );
    assert!(requests.is_empty());
    assert_eq!(environment_dirs(&db, root), vec![dir.to_path_buf()]);
}

#[test]
fn test_sentinel_edit_on_idle_root_skips_rescan() {
    let tmp = tempfile::tempdir().unwrap();
    let rprofile = tmp.path().join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];

    fs::write(&rprofile, "options(foo = 1)\n").unwrap();
    let requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&rprofile)], &HashSet::new());
    assert!(requests.is_empty());
    assert_eq!(environment_dirs(&db, root), vec![tmp.path().to_path_buf()]);
}

#[test]
fn test_sentinel_deletion_under_other_case_spelling_rescans() {
    // The scan lists `Sub`, but the deletion arrives as `sub/.Rprofile`. The
    // event's directory looks unlisted and sentinel-free, which must not be
    // mistaken for an up-to-date listing.
    let tmp = tempfile::tempdir().unwrap();
    if !is_case_insensitive(tmp.path()) {
        return;
    }
    let sub = tmp.path().join("Sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join(".Rprofile"), "").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];
    assert_eq!(environment_dirs(&db, root), vec![sub.clone()]);

    fs::remove_file(sub.join(".Rprofile")).unwrap();
    let rprofile = tmp.path().join("sub/.Rprofile");
    let requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&rprofile)], &HashSet::new());
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());
    assert!(environment_dirs(&db, root).is_empty());
}

#[test]
fn test_sentinel_event_under_stale_case_spelling_rescans() {
    // The scan lists `Sub`, which is then renamed to `sub`. An event under the
    // old spelling finds `Sub` listed and its sentinel still resolves, but the
    // next scan would list `sub`.
    let tmp = tempfile::tempdir().unwrap();
    if !is_case_insensitive(tmp.path()) {
        return;
    }
    let old = tmp.path().join("Sub");
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join(".Rprofile"), "").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];
    assert_eq!(environment_dirs(&db, root), vec![old.clone()]);

    let new = tmp.path().join("sub");
    fs::rename(&old, &new).unwrap();
    let rprofile = old.join(".Rprofile");
    let requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&rprofile)], &HashSet::new());
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());
    assert_eq!(environment_dirs(&db, root), vec![new]);
}

#[cfg(unix)]
#[test]
fn test_environment_dir_replaced_by_symlink_rescans() {
    // The walker doesn't follow symlinks, so `sub` stops being an environment
    // directory even though its sentinel is still reachable through the link.
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let sub = ws.join("sub");
    let target = tmp.path().join("target");
    fs::create_dir_all(&sub).unwrap();
    fs::create_dir_all(&target).unwrap();
    fs::write(sub.join(".Rprofile"), "").unwrap();
    fs::write(target.join(".Rprofile"), "").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[ws])[0];
    assert_eq!(environment_dirs(&db, root), vec![sub.clone()]);

    fs::remove_dir_all(&sub).unwrap();
    std::os::unix::fs::symlink(&target, &sub).unwrap();
    let requests = scheduler.apply_watcher_events(
        &mut db,
        vec![watched_path(&sub.join(".Rprofile"))],
        &HashSet::new(),
    );
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());
    assert!(environment_dirs(&db, root).is_empty());
}

#[test]
fn test_sentinel_edit_during_scan_queues_rescan() {
    // `environment_dirs()` matches the disk, but the in-flight scan's result
    // replaces it, so the event can't be judged against it.
    let tmp = tempfile::tempdir().unwrap();
    let rprofile = tmp.path().join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];

    let description = tmp.path().join("DESCRIPTION");
    fs::write(&description, "Package: pkg\nVersion: 0.0.0\n").unwrap();
    let mut requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&description)], &HashSet::new());
    assert_eq!(requests.len(), 1);
    let request = requests.pop().unwrap();

    let requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&rprofile)], &HashSet::new());
    assert!(requests.is_empty());

    let followups = scheduler.apply_scan_completed(&mut db, request.run(), &HashSet::new());
    assert_eq!(followups.len(), 1);
    assert_eq!(followups[0].root, root);
}

#[test]
fn test_sentinel_event_during_scan_queues_rescan() {
    // The in-flight scan ran before the sentinel was written, so only the
    // queued rescan can see it.
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let request = scheduler
        .set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new())
        .pop()
        .unwrap();
    let root = request.root;
    let result = request.run();

    let rprofile = tmp.path().join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&rprofile)], &HashSet::new());
    assert!(requests.is_empty());

    let followups = scheduler.apply_scan_completed(&mut db, result, &HashSet::new());
    assert!(environment_dirs(&db, root).is_empty());
    assert_eq!(followups.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, followups, &HashSet::new());
    assert_eq!(environment_dirs(&db, root), vec![tmp.path().to_path_buf()]);
}

#[test]
fn test_sentinel_and_r_file_events_in_one_batch() {
    // The R-file event precedes the sentinel in the batch, but must still
    // buffer until the sentinel's rescan finishes.
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];

    let rprofile = tmp.path().join(".Rprofile");
    let script = tmp.path().join("script.R");
    fs::write(&rprofile, "").unwrap();
    fs::write(&script, "x <- 1\n").unwrap();
    let requests = scheduler.apply_watcher_events(
        &mut db,
        vec![watched_path(&script), watched_path(&rprofile)],
        &HashSet::new(),
    );
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());

    assert_eq!(environment_dirs(&db, root), vec![tmp.path().to_path_buf()]);
    let script = FilePath::from_path_buf(script).unwrap();
    assert!(db.file_by_path(&script).is_some());
}

#[test]
fn test_editor_owned_sentinel_still_rescans() {
    // The editor owns the contents of an open `.Rprofile`, but whether the
    // file exists is still read from disk.
    let tmp = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];

    let rprofile = tmp.path().join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let skip = HashSet::from([FilePath::from_path_buf(rprofile.clone()).unwrap()]);
    let requests = scheduler.apply_watcher_events(&mut db, vec![watched_path(&rprofile)], &skip);
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &skip);
    assert_eq!(environment_dirs(&db, root), vec![tmp.path().to_path_buf()]);
}

// --- Open buffers missing on disk ---

fn rescan_without_open_file(
    db: &mut OakDatabase,
    scheduler: &mut ScanScheduler,
    tmp: &Path,
    file_path: &Path,
) -> (oak_db::File, HashSet<FilePath>) {
    let path = watched_path(file_path);
    let file = db.upsert_editor(path.clone(), "edited <- 1\n".to_string());
    let editor_owned = HashSet::from([path]);

    fs::remove_file(file_path).unwrap();
    let rprofile = tmp.join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let requests = scheduler.apply_watcher_events(db, vec![watched_path(&rprofile)], &editor_owned);
    assert_eq!(requests.len(), 1);
    drain_scheduler(db, scheduler, requests, &editor_owned);

    (file, editor_owned)
}

#[test]
fn test_rescan_moves_open_script_missing_on_disk_to_orphan() {
    let tmp = tempfile::tempdir().unwrap();
    let script = tmp.path().join("a.R");
    fs::write(&script, "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];

    let (file, _) = rescan_without_open_file(&mut db, &mut scheduler, tmp.path(), &script);
    let path = watched_path(&script);

    assert!(root.scripts(&db).is_empty());
    assert_eq!(db.file_by_path(&path), Some(file));
    assert!(db.orphan_root().files(&db).contains(&file));

    // `did_change()` passes the updated buffer to `upsert_editor()`, which must
    // reuse the `File` held by the editor even though it is missing on disk.
    for contents in ["edited <- 2\n", "edited <- 3\n"] {
        assert_eq!(db.upsert_editor(path.clone(), contents.to_string()), file);
        assert_eq!(file.source_text(&db), contents);
    }
}

#[test]
fn test_rescan_moves_open_package_file_missing_on_disk_to_orphan() {
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[
        ("a.R", "x <- 1\n"),
        ("b.R", "y <- 1\n"),
    ]);
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];
    let pkg = root.packages(&db)[0];

    let a = tmp.path().join("pkg/R/a.R");
    let (file, _) = rescan_without_open_file(&mut db, &mut scheduler, tmp.path(), &a);
    let path = watched_path(&a);

    assert_eq!(root.packages(&db).clone(), vec![pkg]);
    assert!(!pkg.files(&db).contains(&file));
    assert_eq!(file.package(&db), None);
    assert_eq!(db.file_by_path(&path), Some(file));
    assert!(db.orphan_root().files(&db).contains(&file));

    for contents in ["edited <- 2\n", "edited <- 3\n"] {
        assert_eq!(db.upsert_editor(path.clone(), contents.to_string()), file);
        assert_eq!(file.source_text(&db), contents);
    }
}

#[test]
fn test_rescan_restores_orphaned_open_file_when_it_reappears_on_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let script = tmp.path().join("a.R");
    fs::write(&script, "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];

    let (file, editor_owned) =
        rescan_without_open_file(&mut db, &mut scheduler, tmp.path(), &script);

    fs::write(&script, "edited <- 1\n").unwrap();
    let rprofile = tmp.path().join(".Rprofile");
    fs::remove_file(&rprofile).unwrap();
    let requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&rprofile)], &editor_owned);
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &editor_owned);

    assert_eq!(root.scripts(&db).clone(), vec![file]);
    assert!(!db.orphan_root().files(&db).contains(&file));
}

#[test]
fn test_description_deletion_keeps_open_package_file_entity() {
    // Files must retain their `File` identity when `DESCRIPTION` deletion turns
    // them into workspace scripts, preserving editor references and query caches.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[
        ("a.R", "x <- 1\n"),
        ("b.R", "y <- 1\n"),
    ]);
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];

    let a = watched_path(&tmp.path().join("pkg/R/a.R"));
    let b = db
        .file_by_path(&watched_path(&tmp.path().join("pkg/R/b.R")))
        .unwrap();
    let file = db.upsert_editor(a.clone(), "edited <- 1\n".to_string());
    let editor_owned = HashSet::from([a.clone()]);

    let description = tmp.path().join("pkg/DESCRIPTION");
    fs::remove_file(&description).unwrap();
    let requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&description)], &editor_owned);
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &editor_owned);

    assert!(root.packages(&db).is_empty());
    let scripts: HashSet<_> = root.scripts(&db).iter().copied().collect();
    assert_eq!(scripts, HashSet::from([file, b]));
    assert_eq!(db.file_by_path(&a), Some(file));
    assert_eq!(file.package(&db), None);
    assert_eq!(file.source_text(&db), "edited <- 1\n");
    assert!(!db.orphan_root().files(&db).contains(&file));
}

#[test]
fn test_nested_root_scan_moves_open_file_missing_on_disk_to_orphan() {
    // The nested root has no files before its first scan, but the scan updates
    // the outer root's shared `Package`. Capturing only the nested root's files
    // would miss the open file removed from that package.
    let tmp = tempfile::tempdir().unwrap();
    let pkg_dir = tmp.path().join("pkg");
    write_package(&pkg_dir, "pkg", &[("a.R", "x <- 1\n"), ("b.R", "y <- 1\n")]);
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let outer = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];
    let pkg = outer.packages(&db)[0];

    let a = pkg_dir.join("R/a.R");
    let path = watched_path(&a);
    let file = db.upsert_editor(path.clone(), "edited <- 1\n".to_string());
    let editor_owned = HashSet::from([path.clone()]);
    fs::remove_file(&a).unwrap();

    let paths = [tmp.path().to_path_buf(), pkg_dir];
    let requests = scheduler.set_workspace_paths(&mut db, &paths, &editor_owned);
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &editor_owned);

    let inner = db.workspace_roots().roots(&db)[1];
    assert_eq!(inner.packages(&db).clone(), vec![pkg]);
    assert!(!pkg.files(&db).contains(&file));
    assert_eq!(file.package(&db), None);
    assert_eq!(db.file_by_path(&path), Some(file));
    assert!(db.orphan_root().files(&db).contains(&file));
    assert_eq!(db.upsert_editor(path, "edited <- 2\n".to_string()), file);
}

#[test]
fn test_parent_description_keeps_open_child_package_file_entity() {
    // The parent package's walk also lists the child's files as scripts. The
    // parent is upserted first and takes the open file from the child, so the
    // child must still find the same `File` when it takes it back.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("child"), "child", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let root = scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()])[0];
    let child = root.packages(&db)[0];

    let path = watched_path(&tmp.path().join("child/R/a.R"));
    let file = db.upsert_editor(path.clone(), "edited <- 1\n".to_string());
    let editor_owned = HashSet::from([path.clone()]);

    let description = tmp.path().join("DESCRIPTION");
    fs::write(&description, "Package: parent\nVersion: 0.0.0\n").unwrap();
    let requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&description)], &editor_owned);
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &editor_owned);

    let packages = root.packages(&db).clone();
    assert_eq!(packages.len(), 2);
    assert_eq!(packages[0].name(&db), "parent");
    assert_eq!(packages[1], child);
    assert!(packages[0].scripts(&db).is_empty());
    assert_eq!(child.files(&db), &[file]);
    assert_eq!(file.package(&db), Some(child));
    assert_eq!(db.file_by_path(&path), Some(file));
    assert_eq!(file.source_text(&db), "edited <- 1\n");
}

#[test]
fn test_rescued_open_file_leaves_stale() {
    // Removing the outer root moves its scripts to stale, including the one the
    // inner root still lists. Opening that file finds the live entity and
    // leaves it in stale. When the inner root's rescan then drops it, the
    // rescue must take it out of stale.
    let tmp = tempfile::tempdir().unwrap();
    let outer = tmp.path().join("outer");
    let inner = outer.join("inner");
    fs::create_dir_all(&inner).unwrap();
    let script = inner.join("s.R");
    fs::write(&script, "x <- 1\n").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    scan_workspace(&mut db, &mut scheduler, &[outer, inner.clone()]);

    let path = watched_path(&script);
    let requests = scheduler.set_workspace_paths(&mut db, &[inner], &HashSet::new());
    assert_eq!(requests.len(), 1);
    let file = db.file_by_path(&path).unwrap();
    assert!(db.stale_root().files(&db).contains(&file));

    assert_eq!(
        db.upsert_editor(path.clone(), "edited <- 1\n".to_string()),
        file
    );
    let editor_owned = HashSet::from([path.clone()]);
    fs::remove_file(&script).unwrap();
    drain_scheduler(&mut db, &mut scheduler, requests, &editor_owned);

    assert_eq!(db.file_by_path(&path), Some(file));
    assert!(db.orphan_root().files(&db).contains(&file));
    assert!(!db.stale_root().files(&db).contains(&file));
}

#[test]
fn test_noop_rescan_with_open_buffer_does_not_bump_salsa_revision() {
    // An open buffer adds lookups before and after the scan, but no writes.
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    fs::write(tmp.path().join("top.R"), "y <- 2\n").unwrap();
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()]);

    let mut editor_owned = HashSet::new();
    for open in ["pkg/R/a.R", "top.R"] {
        let path = watched_path(&tmp.path().join(open));
        db.upsert_editor(path.clone(), "edited <- 1\n".to_string());
        editor_owned.insert(path);
    }

    let before = salsa::plumbing::current_revision(&db);
    let description = watched_path(&tmp.path().join("pkg/DESCRIPTION"));
    let requests = scheduler.apply_watcher_events(&mut db, vec![description], &editor_owned);
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &editor_owned);
    assert_eq!(salsa::plumbing::current_revision(&db), before);
}

/// Return `(tmp, outer_root, inner_root)` regardless of workspace registration order.
fn nested_roots(
    db: &mut OakDatabase,
    scheduler: &mut ScanScheduler,
    inner_first: bool,
) -> (tempfile::TempDir, Root, Root) {
    let tmp = tempfile::tempdir().unwrap();
    let outer = tmp.path().join("outer");
    let inner = outer.join("inner");
    fs::create_dir_all(&inner).unwrap();
    let paths = match inner_first {
        true => vec![inner, outer],
        false => vec![outer, inner],
    };
    let roots = scan_workspace(db, scheduler, &paths);
    match inner_first {
        true => (tmp, roots[1], roots[0]),
        false => (tmp, roots[0], roots[1]),
    }
}

#[test]
fn test_sentinel_event_rescans_every_containing_root() {
    for inner_first in [false, true] {
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();
        let (tmp, outer, inner) = nested_roots(&mut db, &mut scheduler, inner_first);
        let inner_dir = tmp.path().join("outer/inner");

        let rprofile = inner_dir.join(".Rprofile");
        fs::write(&rprofile, "").unwrap();
        let requests =
            scheduler.apply_watcher_events(&mut db, vec![watched_path(&rprofile)], &HashSet::new());
        assert_eq!(requests.len(), 2);
        drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());

        assert_eq!(environment_dirs(&db, inner), vec![inner_dir.clone()]);
        assert_eq!(environment_dirs(&db, outer), vec![inner_dir]);
    }
}

#[test]
fn test_removing_nested_root_keeps_shared_entities_in_survivor() {
    // Both roots list `pkg` and `inner/script.R`. Evicting either one must not
    // leave the survivor with an emptied package or files stuck in stale.
    for remove_outer in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let outer = tmp.path().join("outer");
        let inner = outer.join("inner");
        write_package(&inner.join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
        fs::write(inner.join("script.R"), "y <- 2\n").unwrap();
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();
        let roots = scan_workspace(&mut db, &mut scheduler, &[outer.clone(), inner.clone()]);

        let (survivor_path, survivor) = match remove_outer {
            true => (inner.clone(), roots[1]),
            false => (outer.clone(), roots[0]),
        };
        let requests = scheduler.set_workspace_paths(&mut db, &[survivor_path], &HashSet::new());
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].root, survivor);
        drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());

        let pkg = survivor.packages(&db)[0];
        let a = db
            .file_by_path(&watched_path(&inner.join("pkg/R/a.R")))
            .unwrap();
        let script = db
            .file_by_path(&watched_path(&inner.join("script.R")))
            .unwrap();
        assert_eq!(pkg.files(&db), &[a]);
        assert_eq!(a.package(&db), Some(pkg));
        assert_eq!(survivor.scripts(&db).as_slice(), &[script]);
        assert!(!db.stale_root().files(&db).contains(&a));
        assert!(!db.stale_root().files(&db).contains(&script));
        assert!(!db.stale_root().packages(&db).contains(&pkg));
    }
}

#[test]
fn test_removing_nested_root_keeps_shared_scripts_in_survivor() {
    for remove_outer in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("script.R"), "y <- 2\n").unwrap();
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();
        let roots = scan_workspace(&mut db, &mut scheduler, &[
            tmp.path().to_path_buf(),
            sub.clone(),
        ]);

        let (survivor_path, survivor) = match remove_outer {
            true => (sub.clone(), roots[1]),
            false => (tmp.path().to_path_buf(), roots[0]),
        };
        let requests = scheduler.set_workspace_paths(&mut db, &[survivor_path], &HashSet::new());
        assert_eq!(requests.len(), 1);
        drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());

        let script = db
            .file_by_path(&watched_path(&sub.join("script.R")))
            .unwrap();
        assert_eq!(survivor.scripts(&db).as_slice(), &[script]);
        assert!(!db.stale_root().files(&db).contains(&script));
    }
}

#[test]
fn test_removing_nested_root_repairs_file_held_in_different_containers() {
    // With roots `pkg` and `pkg/R`, the outer root lists `a.R` in `pkg.files`
    // and the inner root, which can't see `DESCRIPTION`, in its `scripts`.
    for remove_outer in [false, true] {
        let tmp = tempfile::tempdir().unwrap();
        let outer = tmp.path().join("pkg");
        let inner = outer.join("R");
        write_package(&outer, "pkg", &[("a.R", "x <- 1\n")]);
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();

        // Apply the outer scan last, so the shared `File` points at the package.
        let requests = scheduler.set_workspace_paths(
            &mut db,
            &[outer.clone(), inner.clone()],
            &HashSet::new(),
        );
        let roots = db.workspace_roots().roots(&db).clone();
        let (outer_root, inner_root) = (roots[0], roots[1]);
        let (outer_req, inner_req): (Vec<_>, Vec<_>) =
            requests.into_iter().partition(|req| req.root == outer_root);
        drain_scheduler(&mut db, &mut scheduler, inner_req, &HashSet::new());
        drain_scheduler(&mut db, &mut scheduler, outer_req, &HashSet::new());

        let pkg = outer_root.packages(&db)[0];
        let a = db.file_by_path(&watched_path(&inner.join("a.R"))).unwrap();
        assert_eq!(pkg.files(&db), &[a]);
        assert_eq!(inner_root.scripts(&db).as_slice(), &[a]);
        assert_eq!(a.package(&db), Some(pkg));

        let survivor_path = if remove_outer {
            inner.clone()
        } else {
            outer.clone()
        };
        let requests = scheduler.set_workspace_paths(&mut db, &[survivor_path], &HashSet::new());
        assert_eq!(requests.len(), 1);
        drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());

        if remove_outer {
            assert_eq!(inner_root.scripts(&db).as_slice(), &[a]);
        } else {
            assert_eq!(pkg.files(&db), &[a]);
            assert_eq!(a.package(&db), Some(pkg));
        }
        assert!(!db.stale_root().files(&db).contains(&a));
    }
}

#[test]
fn test_r_file_event_waits_for_every_containing_root_scan() {
    // `new.R` must survive both scan results, whichever root completes first.
    // Both scans predate the file and replace the shared package's `files`.
    for inner_first in [false, true] {
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();
        let (tmp, outer, inner) = nested_roots(&mut db, &mut scheduler, inner_first);
        let pkg = tmp.path().join("outer/inner/pkg");
        write_package(&pkg, "pkg", &[("a.R", "x <- 1\n")]);
        let requests = scheduler.apply_watcher_events(
            &mut db,
            vec![watched_path(&pkg.join("DESCRIPTION"))],
            &HashSet::new(),
        );
        drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());

        let rprofile = tmp.path().join("outer/inner/.Rprofile");
        fs::write(&rprofile, "").unwrap();
        let requests =
            scheduler.apply_watcher_events(&mut db, vec![watched_path(&rprofile)], &HashSet::new());
        assert_eq!(requests.len(), 2);
        let mut results: Vec<_> = requests.into_iter().map(|req| req.run()).collect();

        let new_file = pkg.join("R/new.R");
        fs::write(&new_file, "y <- 2\n").unwrap();
        let followups =
            scheduler.apply_watcher_events(&mut db, vec![watched_path(&new_file)], &HashSet::new());
        assert!(followups.is_empty());

        let second = results.pop().unwrap();
        let first = results.pop().unwrap();
        let followups = scheduler.apply_scan_completed(&mut db, first, &HashSet::new());
        assert!(followups.is_empty());
        let followups = scheduler.apply_scan_completed(&mut db, second, &HashSet::new());
        assert!(followups.is_empty());
        assert!(!scheduler.has_pending_scans());

        let new_file = FilePath::from_path_buf(new_file).unwrap();
        assert!(db.file_by_path(&new_file).is_some());
        assert_eq!(inner.packages(&db)[0].files(&db).len(), 2);
        assert_eq!(outer.packages(&db)[0].files(&db).len(), 2);
    }
}

#[test]
fn test_buffered_path_matches_disk_after_overlapping_rescans() -> io::Result<()> {
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    let (tmp, _, _) = nested_roots(&mut db, &mut scheduler, false);
    let outer_dir = tmp.path().join("outer");
    let inner_dir = outer_dir.join("inner");
    let script = inner_dir.join("script.R");
    fs::write(&script, "x <- 1\n")?;
    let sentinel = inner_dir.join(".Rprofile");
    fs::write(&sentinel, "")?;
    let requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&sentinel)], &HashSet::new());
    assert_eq!(requests.len(), 2);
    let mut results: Vec<_> = requests.into_iter().map(|request| request.run()).collect();
    let outer_result = results.remove(0);
    let inner_result = results.remove(0);

    fs::remove_file(&script)?;
    assert!(scheduler
        .apply_watcher_events(&mut db, vec![watched_path(&script)], &HashSet::new(),)
        .is_empty());
    assert!(scheduler
        .apply_scan_completed(&mut db, outer_result, &HashSet::new())
        .is_empty());

    // Only the outer root rescans; its snapshot predates the recreation.
    let outer_sentinel = outer_dir.join(".Rprofile");
    fs::write(&outer_sentinel, "")?;
    let mut requests = scheduler.apply_watcher_events(
        &mut db,
        vec![watched_path(&outer_sentinel)],
        &HashSet::new(),
    );
    assert_eq!(requests.len(), 1);
    let outer_result = requests.remove(0).run();
    fs::write(&script, "x <- 2\n")?;
    assert!(scheduler
        .apply_watcher_events(&mut db, vec![watched_path(&script)], &HashSet::new(),)
        .is_empty());
    assert!(scheduler
        .apply_scan_completed(&mut db, inner_result, &HashSet::new())
        .is_empty());
    assert!(scheduler
        .apply_scan_completed(&mut db, outer_result, &HashSet::new())
        .is_empty());

    assert!(!scheduler.has_pending_scans());
    let path = watched_path(&script);
    assert!(db.file_by_path(&path).is_some());
    Ok(())
}

#[test]
fn test_removing_blocking_root_preserves_events_for_surviving_root() -> io::Result<()> {
    for inner_pending in [false, true] {
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();
        let (tmp, _, inner) = nested_roots(&mut db, &mut scheduler, false);
        let outer_dir = tmp.path().join("outer");
        let inner_dir = outer_dir.join("inner");
        let sentinel_dir = if inner_pending {
            &inner_dir
        } else {
            &outer_dir
        };
        let sentinel = sentinel_dir.join(".Rprofile");
        fs::write(&sentinel, "")?;
        let requests =
            scheduler.apply_watcher_events(&mut db, vec![watched_path(&sentinel)], &HashSet::new());
        assert_eq!(requests.len(), if inner_pending { 2 } else { 1 });
        let results: Vec<_> = requests.into_iter().map(|request| request.run()).collect();
        let script = inner_dir.join("new.R");
        fs::write(&script, "x <- 1\n")?;
        let path = watched_path(&script);
        assert!(scheduler
            .apply_watcher_events(&mut db, vec![watched_path(&script)], &HashSet::new(),)
            .is_empty());
        assert!(db.file_by_path(&path).is_none());

        assert!(scheduler
            .set_workspace_paths(&mut db, &[inner_dir], &HashSet::new())
            .is_empty());
        assert_eq!(db.file_by_path(&path).is_some(), !inner_pending);
        for result in results {
            assert!(scheduler
                .apply_scan_completed(&mut db, result, &HashSet::new())
                .is_empty());
        }
        assert!(!scheduler.has_pending_scans());
        assert_eq!(inner.scripts(&db).len(), 1);
        assert_eq!(inner.scripts(&db)[0].path(&db), &path);
        assert!(db.file_by_path(&path).is_some());
    }
    Ok(())
}

#[test]
fn test_blocked_events_do_not_delay_unrelated_roots() -> io::Result<()> {
    let tmp = tempfile::tempdir()?;
    let blocked_dir = tmp.path().join("blocked");
    let ready_dir = tmp.path().join("ready");
    fs::create_dir_all(&blocked_dir)?;
    fs::create_dir_all(&ready_dir)?;
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();
    scan_workspace(&mut db, &mut scheduler, &[
        blocked_dir.clone(),
        ready_dir.clone(),
    ]);
    let sentinel = blocked_dir.join(".Rprofile");
    fs::write(&sentinel, "")?;
    let mut requests =
        scheduler.apply_watcher_events(&mut db, vec![watched_path(&sentinel)], &HashSet::new());
    assert_eq!(requests.len(), 1);
    let result = requests.remove(0).run();
    let blocked_script = blocked_dir.join("new.R");
    let ready_script = ready_dir.join("new.R");
    fs::write(&blocked_script, "x <- 1\n")?;
    fs::write(&ready_script, "y <- 1\n")?;
    assert!(scheduler
        .apply_watcher_events(
            &mut db,
            vec![watched_path(&blocked_script), watched_path(&ready_script),],
            &HashSet::new(),
        )
        .is_empty());
    let blocked_path = watched_path(&blocked_script);
    let ready_path = watched_path(&ready_script);
    assert!(db.file_by_path(&blocked_path).is_none());
    assert!(db.file_by_path(&ready_path).is_some());
    assert!(scheduler.has_pending_scans());

    assert!(scheduler
        .apply_scan_completed(&mut db, result, &HashSet::new())
        .is_empty());
    assert!(db.file_by_path(&blocked_path).is_some());
    assert!(db.file_by_path(&ready_path).is_some());
    assert!(!scheduler.has_pending_scans());
    Ok(())
}

#[test]
fn test_blocked_deletion_survives_editor_open_and_close() -> io::Result<()> {
    // The pending scan listed `script.R` before its deletion. The editor opens
    // and closes the file before that scan completes, so the buffered path
    // must still remove it afterwards. The deletion is reported either while
    // the file is open, or before it opens and an unrelated event drains.
    for owned_at_event in [true, false] {
        let tmp = tempfile::tempdir()?;
        let script = tmp.path().join("script.R");
        fs::write(&script, "x <- 1\n")?;
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();
        scan_workspace(&mut db, &mut scheduler, &[tmp.path().to_path_buf()]);

        let sentinel = tmp.path().join(".Rprofile");
        fs::write(&sentinel, "")?;
        let mut requests =
            scheduler.apply_watcher_events(&mut db, vec![watched_path(&sentinel)], &HashSet::new());
        assert_eq!(requests.len(), 1);
        let result = requests.remove(0).run();

        fs::remove_file(&script)?;
        let open = HashSet::from([watched_path(&script)]);
        let event_skip = if owned_at_event {
            open.clone()
        } else {
            HashSet::new()
        };
        assert!(scheduler
            .apply_watcher_events(&mut db, vec![watched_path(&script)], &event_skip)
            .is_empty());
        assert!(scheduler
            .apply_watcher_events(&mut db, vec![], &open)
            .is_empty());

        assert!(scheduler
            .apply_scan_completed(&mut db, result, &HashSet::new())
            .is_empty());
        assert!(db.file_by_path(&watched_path(&script)).is_none());
    }
    Ok(())
}

#[test]
fn test_description_event_rescans_every_containing_root() {
    for inner_first in [false, true] {
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();
        let (tmp, outer, inner) = nested_roots(&mut db, &mut scheduler, inner_first);

        let pkg = tmp.path().join("outer/inner/pkg");
        write_package(&pkg, "pkg", &[("a.R", "x <- 1\n")]);
        let requests = scheduler.apply_watcher_events(
            &mut db,
            vec![watched_path(&pkg.join("DESCRIPTION"))],
            &HashSet::new(),
        );
        assert_eq!(requests.len(), 2);
        drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());

        assert_eq!(inner.packages(&db).len(), 1);
        assert_eq!(outer.packages(&db).len(), 1);
    }
}

#[test]
fn test_set_workspace_paths_inserts_empty_root_immediately() {
    // While the scan is in flight, the new `Root` is already in
    // `workspace_roots` (empty). This is what lets the watcher
    // scheduler classify events for files in the pending root and
    // buffer them, instead of dropping them as "no workspace
    // contains this URL".
    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", &[("a.R", "x <- 1\n")]);
    let mut db = OakDatabase::new();
    let mut scheduler = ScanScheduler::new();

    let _requests =
        scheduler.set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());

    // Before any scan runs:
    let roots = db.workspace_roots().roots(&db).clone();
    assert_eq!(roots.len(), 1);
    assert!(roots[0].packages(&db).is_empty());
    // `FilePath` construction is lexical, so the stored path is the one
    // we handed in, byte for byte.
    assert_eq!(
        roots[0].path(&db).as_path().unwrap().as_std_path(),
        tmp.path()
    );
}
