//! Integration test that drives the real [`GlobalState`] event loop.
//!
//! Where the handler tests in [`super::state_handlers`] reconstruct the scan
//! pump by hand, this one feeds an event through the production `handle_event`
//! and lets the loop dispatch the scan, run it on a blocking task, route the
//! [`Event::OakScanCompleted`] back, and apply it. So it pins the main loop's
//! own wiring: which arm calls which handler, and the apply-and-redispatch
//! step. The scheduler's policy is unit tested without tokio in `oak_scan`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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
use super::utils::test_client;
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

/// A main-loop Salsa write must never park behind blocking-pool tasks that can't
/// observe cancellation. A task owning a db snapshot pins a hold from the moment it is
/// queued, and a write waits for every hold to drop, so a queued snapshot sitting
/// behind an unbounded source fetch turns the next edit into a deadlock.
///
/// The deadlock is a condvar park inside a salsa setter, which no `tokio::time::timeout`
/// can see. So the script runs on its own thread and reports progress through phase
/// markers that the test thread waits on with a deadline.
#[test]
fn test_main_loop_write_survives_saturated_blocking_pool() {
    // Same caps as production: two workers, two blocking threads.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .max_blocking_threads(2)
        .build()
        .unwrap();

    let (donor1_gate, donor1_entered, donor1_release) = gate();
    let (donor2_gate, donor2_entered, donor2_release) = gate();

    let (phase_tx, phase_rx) = std::sync::mpsc::channel::<()>();
    let handle = rt.handle().clone();

    std::thread::spawn(move || {
        let _aux = init_aux_for_test();

        let handler = Arc::new(TestSourceHandler::new(HashMap::from([
            (String::from("donor1"), TestBehavior::Gated(donor1_gate)),
            (String::from("donor2"), TestBehavior::Gated(donor2_gate)),
        ])));

        let lib = tempfile::tempdir().unwrap();
        for name in ["donor1", "donor2"] {
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
            WorldState::new(db),
            LspState::new(
                tokio::sync::mpsc::unbounded_channel().0,
                SourceScheduler::new(Some(handler)),
            ),
        );

        // A workspace package using both library packages, so the scan hands the
        // scheduler two dependencies to fetch.
        let workspace = tempfile::tempdir().unwrap();
        let myproj = workspace.path().join("myproj");
        DescriptionWriter::new()
            .package("myproj")
            .version("0.0.0")
            .write(&myproj);
        write_sources(&myproj.join("R"), &[(
            "use.R",
            "donor1::foo()\ndonor2::bar()\n",
        )]);
        let script = workspace.path().join("script.R");

        handle.block_on(async move {
            state
                .handle_event_once(did_change_workspace_folders(workspace.path()))
                .await;
            state.pump_scans_to_quiescence().await;
            phase_tx.send(()).unwrap();

            // Both blocking threads are now parked in a fetch that salsa cancellation
            // can't reach. Warmup was spawned before the fetches and the pool is FIFO,
            // so its own hold has already dropped.
            donor1_entered.recv().unwrap();
            donor2_entered.recv().unwrap();
            phase_tx.send(()).unwrap();

            // Goes through, no holds outstanding. Ends the tick by queueing a
            // diagnostics pass whose snapshot can never reach a thread.
            state.handle_event_once(did_open(&script, "x <- 1\n")).await;
            phase_tx.send(()).unwrap();

            // The write that has to drain that pinned hold.
            state
                .handle_event_once(did_change(&script, "x <- 2\n", 1))
                .await;
            phase_tx.send(()).unwrap();
        });
    });

    let phases = [
        ("workspace scans quiesced", Duration::from_secs(30)),
        (
            "both source fetches took a blocking thread",
            Duration::from_secs(30),
        ),
        ("didOpen write completed", Duration::from_secs(30)),
        ("didChange write completed", Duration::from_secs(10)),
    ];
    let stalled = phases
        .iter()
        .find(|(_label, timeout)| phase_rx.recv_timeout(*timeout).is_err())
        .map(|(label, _timeout)| *label);

    // Shut the runtime down before anything can panic. Dropping a `Runtime` waits for
    // its blocking tasks, and the gated fetches only finish once we release them, so a
    // panic while `rt` is alive would hang the test instead of failing it.
    rt.shutdown_background();
    drop(donor1_release);
    drop(donor2_release);

    if let Some(stalled) = stalled {
        panic!("Main loop stalled, never reached: {stalled}");
    }
}
