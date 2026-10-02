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
use std::path::Path;
use std::path::PathBuf;

use aether_path::FilePath;
use oak_db::DbInputs;
use oak_db::OakDatabase;
use oak_db::Root;
use oak_db::SourceDb;

use crate::lookup::package_by_path;
use crate::scheduler::drain_scheduler;
use crate::FileEvent;
use crate::FileEventKind;
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
    let event_followups = scheduler.apply_watcher_events(
        &mut db,
        vec![FileEvent {
            kind: FileEventKind::Created,
            path: new_path.clone(),
        }],
        &HashSet::new(),
    );
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
    let watcher_followups = scheduler.apply_watcher_events(
        &mut db,
        vec![FileEvent {
            kind: FileEventKind::Created,
            path: desc_path,
        }],
        &HashSet::new(),
    );
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
    let followups = scheduler.apply_watcher_events(
        &mut db,
        vec![FileEvent {
            kind: FileEventKind::Created,
            path: desc_path,
        }],
        &HashSet::new(),
    );
    assert_eq!(followups.len(), 1);
    assert_eq!(followups[0].root, root);

    drain_scheduler(&mut db, &mut scheduler, followups, &HashSet::new());
    assert_eq!(root.packages(&db).len(), 1);
}

// --- Environment directories ---

fn event(kind: FileEventKind, path: &Path) -> FileEvent {
    FileEvent {
        kind,
        path: FilePath::from_path_buf(path.to_path_buf()).unwrap(),
    }
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
    let requests = scheduler.apply_watcher_events(
        &mut db,
        vec![event(FileEventKind::Created, &rprofile)],
        &HashSet::new(),
    );
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());
    assert_eq!(environment_dirs(&db, root), vec![sub.clone()]);

    fs::remove_file(&rprofile).unwrap();
    let requests = scheduler.apply_watcher_events(
        &mut db,
        vec![event(FileEventKind::Deleted, &rprofile)],
        &HashSet::new(),
    );
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
        vec![event(FileEventKind::Deleted, &dir.join(".Rprofile"))],
        &HashSet::new(),
    );
    drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());
    assert_eq!(environment_dirs(&db, root), vec![dir.to_path_buf()]);
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
    let requests = scheduler.apply_watcher_events(
        &mut db,
        vec![event(FileEventKind::Created, &rprofile)],
        &HashSet::new(),
    );
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
        vec![
            event(FileEventKind::Created, &script),
            event(FileEventKind::Created, &rprofile),
        ],
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
    let requests = scheduler.apply_watcher_events(
        &mut db,
        vec![event(FileEventKind::Created, &rprofile)],
        &skip,
    );
    assert_eq!(requests.len(), 1);
    drain_scheduler(&mut db, &mut scheduler, requests, &skip);
    assert_eq!(environment_dirs(&db, root), vec![tmp.path().to_path_buf()]);
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
        let requests = scheduler.apply_watcher_events(
            &mut db,
            vec![event(FileEventKind::Created, &rprofile)],
            &HashSet::new(),
        );
        assert_eq!(requests.len(), 2);
        drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());

        assert_eq!(environment_dirs(&db, inner), vec![inner_dir.clone()]);
        assert_eq!(environment_dirs(&db, outer), vec![inner_dir]);
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
            vec![event(FileEventKind::Created, &pkg.join("DESCRIPTION"))],
            &HashSet::new(),
        );
        drain_scheduler(&mut db, &mut scheduler, requests, &HashSet::new());

        let rprofile = tmp.path().join("outer/inner/.Rprofile");
        fs::write(&rprofile, "").unwrap();
        let requests = scheduler.apply_watcher_events(
            &mut db,
            vec![event(FileEventKind::Created, &rprofile)],
            &HashSet::new(),
        );
        assert_eq!(requests.len(), 2);
        let mut results: Vec<_> = requests.into_iter().map(|req| req.run()).collect();

        let new_file = pkg.join("R/new.R");
        fs::write(&new_file, "y <- 2\n").unwrap();
        let followups = scheduler.apply_watcher_events(
            &mut db,
            vec![event(FileEventKind::Created, &new_file)],
            &HashSet::new(),
        );
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
fn test_description_event_rescans_every_containing_root() {
    for inner_first in [false, true] {
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();
        let (tmp, outer, inner) = nested_roots(&mut db, &mut scheduler, inner_first);

        let pkg = tmp.path().join("outer/inner/pkg");
        write_package(&pkg, "pkg", &[("a.R", "x <- 1\n")]);
        let requests = scheduler.apply_watcher_events(
            &mut db,
            vec![event(FileEventKind::Created, &pkg.join("DESCRIPTION"))],
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
