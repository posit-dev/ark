//! End-to-end smoke test of the async workspace scan path.
//!
//! Exercises the real tokio plumbing the LSP main loop relies on:
//! [`dispatch_scan_requests`] spawning [`ScanRequest::run`] on a
//! blocking task, the mpsc round-trip back as [`Event::OakScanCompleted`],
//! and the main-loop apply step. The rest of the scheduler is unit
//! tested without tokio in `oak_scan`; this test pins the wiring.

use std::collections::HashSet;
use std::fs;
use std::time::Duration;

use oak_db::DbInputs;
use oak_db::OakDatabase;
use oak_scan::ScanScheduler;
use tokio::runtime::Runtime;
use tokio::time::timeout;

use crate::lsp::main_loop::dispatch_scan_requests;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::Event;

#[test]
fn test_workspace_scan_round_trip_through_tokio() {
    // `dispatch_scan_requests` routes through `lsp::spawn_blocking`, which
    // hands the `JoinHandle` to the aux loop. Without an aux tx the spawn
    // helper drops the panic-logging path; init a no-op aux for the test.
    init_aux_for_test();

    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("pkg/R")).unwrap();
    fs::write(
        tmp.path().join("pkg/DESCRIPTION"),
        "Package: pkg\nVersion: 0.0.0\n",
    )
    .unwrap();
    fs::write(tmp.path().join("pkg/R/a.R"), "x <- 1\n").unwrap();

    let rt = Runtime::new().unwrap();
    rt.block_on(async {
        let mut db = OakDatabase::new();
        let mut scheduler = ScanScheduler::new();
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

        // Kick off the scan as `did_change_workspace_folders` would.
        let initial =
            scheduler.set_workspace_paths(&mut db, &[tmp.path().to_path_buf()], &HashSet::new());
        assert_eq!(initial.len(), 1);
        dispatch_scan_requests(&events_tx, initial);

        // Pump events until the scheduler stops issuing followups. Each
        // iteration exercises the real spawn_blocking + mpsc + apply
        // round-trip the main loop performs in production.
        loop {
            let event = timeout(Duration::from_secs(5), events_rx.recv())
                .await
                .expect("scan timed out")
                .expect("event channel closed");
            let Event::OakScanCompleted(scan) = event else {
                panic!("unexpected event variant");
            };
            let followups = scheduler.apply_scan_completed(&mut db, scan, &HashSet::new());
            if followups.is_empty() {
                break;
            }
            dispatch_scan_requests(&events_tx, followups);
        }

        // Workspace state reflects the on-disk package.
        let roots = db.workspace_roots().roots(&db).clone();
        assert_eq!(roots.len(), 1);
        let packages = roots[0].packages(&db).clone();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name(&db), "pkg");
        assert_eq!(packages[0].files(&db).len(), 1);
    });
}
