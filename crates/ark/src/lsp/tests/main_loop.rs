//! Integration test that drives the real [`GlobalState`] event loop.
//!
//! Where the handler tests in [`super::state_handlers`] reconstruct the scan
//! pump by hand, this one feeds an event through the production `handle_event`
//! and lets the loop dispatch the scan, run it on the scan pool, route the
//! [`Event::OakScanCompleted`] back, and apply it. So it pins the main loop's
//! own wiring: which arm calls which handler, and the apply-and-redispatch
//! step. The scheduler's policy is unit tested without tokio in `oak_scan`.

use std::collections::HashMap;
use std::sync::Arc;

use oak_db::DbInputs;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use tower_lsp_server::ls_types::DidChangeWorkspaceFoldersParams;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::ls_types::WorkspaceFolder;
use tower_lsp_server::ls_types::WorkspaceFoldersChangeEvent;

use super::source_handler::gate;
use super::source_handler::TestBehavior;
use super::source_handler::TestSourceHandler;
use super::utils::did_change;
use super::utils::did_change_workspace_folders;
use super::utils::did_open;
use super::utils::source_scheduler_for_test;
use super::utils::test_client;
use super::utils::world_with_source_fetching;
use super::utils::write_sources;
use super::utils::DescriptionWriter;
use crate::lsp::backend::LspMessage;
use crate::lsp::backend::LspNotification;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::sources::SourceScheduler;
use crate::lsp::state::WorldState;

/// Drive `didChangeWorkspaceFolders` through the real `handle_event`, including
/// the real `OakScanCompleted` arm, to check that the main loop wires scan
/// dispatch and completion-apply together.
#[tokio::test]
async fn test_workspace_folder_scan_drives_through_main_loop() {
    let _aux = init_aux_for_test();
    let mut state = GlobalState::from_parts(
        test_client(),
        WorldState::new(OakDatabase::new()),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            SourceScheduler::new(None),
        ),
    );

    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("pkg");
    DescriptionWriter::new()
        .package("pkg")
        .version("0.0.0")
        .write(&pkg);
    write_sources(&pkg.join("R"), &[("a.R", "x <- 1\n")]);

    let params = DidChangeWorkspaceFoldersParams {
        event: WorkspaceFoldersChangeEvent {
            added: vec![WorkspaceFolder {
                uri: Uri::from_file_path(tmp.path()).unwrap(),
                name: String::new(),
            }],
            removed: vec![],
        },
    };
    state
        .handle_event_to_quiescence(Event::Lsp(LspMessage::Notification(
            LspNotification::DidChangeWorkspaceFolders(params),
        )))
        .await;

    let db = &state.world().db;
    let roots = db.workspace_roots().roots(db).clone();
    assert_eq!(roots.len(), 1);
    let packages = roots[0].packages(db);
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].name(db), "pkg");
    assert_eq!(packages[0].files(db).len(), 1);
}

/// Db-holding work (diagnostics, index warmup) and unbounded I/O (package source
/// fetches) run on separate executors, so saturating the source pool can't stall a
/// main-loop write: the analysis pool stays free to drain the queued diagnostics
/// snapshot the write is waiting on. Gates five packages, one more than
/// `MAX_ANALYSIS_THREADS`, so a regression that merged the two executors back together
/// would leave every thread parked instead of a few free ones, and the write would park
/// behind the pinned snapshot. If that happens, the watchdog (`crate::lsp::watchdog`)
/// aborts the test with a diagnosis instead of hanging to the harness timeout.
#[tokio::test]
async fn test_main_loop_write_survives_saturated_source_pool() {
    let _aux = init_aux_for_test();

    // One shared "entered" sender: with five gates and a 2-thread source pool, only two
    // can ever be inside `handle()` at once, but which two is unpredictable.
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let donors = ["donor1", "donor2", "donor3", "donor4", "donor5"];

    let mut behavior = HashMap::new();
    let mut releases = Vec::new();
    for name in donors {
        let (gate, release) = gate(entered_tx.clone());
        behavior.insert(name.to_string(), TestBehavior::Gated(gate));
        releases.push(release);
    }
    let handler = Arc::new(TestSourceHandler::new(behavior));

    let lib = tempfile::tempdir().unwrap();
    for name in donors {
        DescriptionWriter::new()
            .package(name)
            .version("0.0.0")
            .built("dummy")
            .write(&lib.path().join(name));
    }
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);

    let mut state = GlobalState::from_parts(
        test_client(),
        world_with_source_fetching(db, true),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            source_scheduler_for_test(handler),
        ),
    );

    // A workspace package using all five library packages via `::`, so the scan hands
    // the scheduler five dependencies to fetch.
    let workspace = tempfile::tempdir().unwrap();
    let myproj = workspace.path().join("myproj");
    DescriptionWriter::new()
        .package("myproj")
        .version("0.0.0")
        .write(&myproj);
    let uses: String = donors
        .iter()
        .map(|name| format!("{name}::foo()\n"))
        .collect();
    write_sources(&myproj.join("R"), &[("use.R", &uses)]);
    let script = workspace.path().join("script.R");

    state
        .handle_event_once(did_change_workspace_folders(workspace.path()))
        .await;
    state.pump_scans_to_quiescence().await;

    // The source pool workers are now parked in a fetch that salsa cancellation can't
    // reach. Index warmup went to the analysis pool, so it isn't queued behind them.
    for _ in 0..2 {
        entered_rx.recv().unwrap();
    }

    // Goes through, no holds outstanding. Ends the tick by queueing a diagnostics
    // pass, which needs an analysis thread to run on.
    state.handle_event_once(did_open(&script, "x <- 1\n")).await;

    // The write that has to drain that pinned hold.
    state
        .handle_event_once(did_change(&script, "x <- 2\n", 1))
        .await;

    // Let the still-gated workers finish so the test process can exit cleanly.
    drop(releases);
}
