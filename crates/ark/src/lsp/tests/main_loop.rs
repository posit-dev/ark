//! Integration test that drives the real [`GlobalState`] event loop.
//!
//! Where the handler tests in [`super::state_handlers`] reconstruct the scan
//! pump by hand, this one feeds an event through the production `handle_event`
//! and lets the loop dispatch the scan, run it on a blocking task, route the
//! [`Event::OakScanCompleted`] back, and apply it. So it pins the main loop's
//! own wiring: which arm calls which handler, and the apply-and-redispatch
//! step. The scheduler's policy is unit tested without tokio in `oak_scan`.

use std::path::Path;

use oak_db::DbInputs;
use tower_lsp::lsp_types::DidChangeWorkspaceFoldersParams;
use tower_lsp::lsp_types::InitializeParams;
use tower_lsp::lsp_types::InitializeResult;
use tower_lsp::lsp_types::WorkspaceFolder;
use tower_lsp::lsp_types::WorkspaceFoldersChangeEvent;
use tower_lsp::Client;
use tower_lsp::LanguageServer;
use tower_lsp::LspService;
use url::Url;

use crate::lsp::backend::LspMessage;
use crate::lsp::backend::LspNotification;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;

/// Get a real `Client` without a live connection. `LspService::new` hands a
/// `Client` to its init closure; we capture it and drop the service. The
/// client's sends go nowhere, which is fine since the event paths under test
/// never use it.
fn test_client() -> Client {
    struct Dummy;

    #[tower_lsp::async_trait]
    impl LanguageServer for Dummy {
        async fn initialize(
            &self,
            _: InitializeParams,
        ) -> tower_lsp::jsonrpc::Result<InitializeResult> {
            Ok(InitializeResult::default())
        }
        async fn shutdown(&self) -> tower_lsp::jsonrpc::Result<()> {
            Ok(())
        }
    }

    let captured = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sink = std::sync::Arc::clone(&captured);
    let (_service, _socket) = LspService::new(move |client| {
        *sink.lock().unwrap() = Some(client);
        Dummy
    });

    // Bind first so the `MutexGuard` temporary drops at the `;`, not at the
    // end of the block.
    let client = captured.lock().unwrap().take();
    client.unwrap()
}

fn write_package(dir: &Path, name: &str, basename: &str, contents: &str) {
    std::fs::create_dir_all(dir.join("R")).unwrap();
    std::fs::write(
        dir.join("DESCRIPTION"),
        format!("Package: {name}\nVersion: 0.0.0\n"),
    )
    .unwrap();
    std::fs::write(dir.join("R").join(basename), contents).unwrap();
}

/// Drive `didChangeWorkspaceFolders` through the real `handle_event`, including
/// the real `OakScanCompleted` arm, to check that the main loop wires scan
/// dispatch and completion-apply together.
#[tokio::test]
async fn test_workspace_folder_scan_drives_through_main_loop() {
    let _aux = init_aux_for_test();
    let mut state = GlobalState::new_test(test_client());

    let tmp = tempfile::tempdir().unwrap();
    write_package(&tmp.path().join("pkg"), "pkg", "a.R", "x <- 1\n");

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
