mod description_writer;
mod events;
mod namespace_writer;

use std::path::Path;

use aether_path::FilePath;
pub(super) use description_writer::DescriptionWriter;
pub(super) use events::did_change_workspace_folders;
pub(super) use events::did_open;
pub(super) use namespace_writer::NamespaceWriter;
use oak_scan::DbScan;
use tower_lsp::lsp_types;
use tower_lsp::Client;
use tower_lsp::LanguageServer;
use tower_lsp::LspService;

use crate::lsp::state::WorldState;

/// Get a real `Client` without a live connection. `LspService::new` hands a
/// `Client` to its init closure; we capture it and drop the service. The
/// client's sends go nowhere, which is fine since the event paths under test
/// never use it.
pub(super) fn test_client() -> Client {
    struct Dummy;

    #[tower_lsp::async_trait]
    impl LanguageServer for Dummy {
        async fn initialize(
            &self,
            _: lsp_types::InitializeParams,
        ) -> tower_lsp::jsonrpc::Result<lsp_types::InitializeResult> {
            Ok(lsp_types::InitializeResult::default())
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

pub(super) fn write_sources(dir: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(dir).unwrap();
    for (basename, contents) in files {
        std::fs::write(dir.join(basename), contents).unwrap();
    }
}

pub(super) fn make_state(uri: &lsp_types::Url, contents: &str) -> WorldState {
    let mut state = WorldState::default();
    insert_file(&mut state, uri, contents);
    state
}

/// Insert an editor buffer, the same as `did_open` performs, so handlers
/// reading either `state.documents` or `state.db` (via `file_by_path`) see a
/// consistent file.
pub(super) fn insert_file(state: &mut WorldState, uri: &lsp_types::Url, contents: &str) {
    let file = state
        .db
        .upsert_editor(FilePath::from_url(uri), contents.to_string());
    state.insert_open_file(uri.clone(), file, None);
}

pub(super) fn range(start: (u32, u32), end: (u32, u32)) -> lsp_types::Range {
    lsp_types::Range {
        start: lsp_types::Position::new(start.0, start.1),
        end: lsp_types::Position::new(end.0, end.1),
    }
}
