//! Integration test that drives the real [`GlobalState`] event loop.
//!
//! Where the handler tests in [`super::state_handlers`] reconstruct the scan
//! pump by hand, this one feeds an event through the production `handle_event`
//! and lets the loop dispatch the scan, run it on a blocking task, route the
//! [`Event::OakScanCompleted`] back, and apply it. So it pins the main loop's
//! own wiring: which arm calls which handler, and the apply-and-redispatch
//! step. The scheduler's policy is unit tested without tokio in `oak_scan`.

use oak_db::DbInputs;
use oak_db::OakDatabase;
use oak_semantic::library::Library;
use tower_lsp::lsp_types::DidChangeWorkspaceFoldersParams;
use tower_lsp::lsp_types::WorkspaceFolder;
use tower_lsp::lsp_types::WorkspaceFoldersChangeEvent;
use url::Url;

use super::utils::test_client;
use super::utils::write_description;
use super::utils::write_sources;
use crate::lsp::backend::LspMessage;
use crate::lsp::backend::LspNotification;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::sources::SourceManager;
use crate::lsp::state::WorldState;

/// Drive `didChangeWorkspaceFolders` through the real `handle_event`, including
/// the real `OakScanCompleted` arm, to check that the main loop wires scan
/// dispatch and completion-apply together.
#[tokio::test]
async fn test_workspace_folder_scan_drives_through_main_loop() {
    let _aux = init_aux_for_test();
    let mut state = GlobalState::from_parts(
        test_client(),
        WorldState::new(OakDatabase::new(), Library::new(vec![])),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            SourceManager::new(None),
        ),
    );

    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("pkg");
    write_description(&pkg, "pkg");
    write_sources(&pkg.join("R"), &[("a.R", "x <- 1\n")]);

    let params = DidChangeWorkspaceFoldersParams {
        event: WorkspaceFoldersChangeEvent {
            added: vec![WorkspaceFolder {
                uri: Url::from_file_path(tmp.path()).unwrap(),
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
