mod description_writer;
mod events;
mod namespace_writer;

use std::path::Path;

use aether_path::FilePath;
pub(super) use description_writer::DescriptionWriter;
pub(super) use events::did_change;
pub(super) use events::did_change_workspace_folders;
pub(super) use events::did_open;
pub(super) use namespace_writer::NamespaceWriter;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::Client;
use tower_lsp_server::LanguageServer;
use tower_lsp_server::LspService;

use crate::lsp::state::WorldState;
use crate::lsp::traits::url::UriExt;

/// Get a real `Client` without a live connection. `LspService::new` hands a
/// `Client` to its init closure; we capture it and drop the service. The
/// client's sends go nowhere, which is fine since the event paths under test
/// never use it.
pub(super) fn test_client() -> Client {
    struct Dummy;

    impl LanguageServer for Dummy {
        async fn initialize(
            &self,
            _: lsp_types::InitializeParams,
        ) -> tower_lsp_server::jsonrpc::Result<lsp_types::InitializeResult> {
            Ok(lsp_types::InitializeResult::default())
        }
        async fn shutdown(&self) -> tower_lsp_server::jsonrpc::Result<()> {
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

/// A [`WorldState`] with source fetching pinned on.
///
/// `WorldState::new()` resolves `OAK_SOURCE_FETCHING_ENABLED`, so a test that
/// drives a fetch pins the setting instead of inheriting the default. Otherwise
/// exporting the variable to save bandwidth locally would decide what these
/// tests exercise.
pub(super) fn world_with_source_fetching(db: OakDatabase) -> WorldState {
    let mut world = WorldState::new(db);
    world.config.oak.source_fetching_enabled = true;
    world
}

pub(super) fn make_state(wire: &str, contents: &str) -> (WorldState, Uri) {
    let mut state = WorldState::default();
    let uri = insert_file(&mut state, wire, contents);
    (state, uri)
}

/// Insert an editor buffer, the same as `did_open` performs, so handlers
/// reading either `state.documents` or `state.db` (via `file_by_path`) see a
/// consistent file.
///
/// Starts from `wire`, the raw bytes an editor would send, rather than a
/// `Url`, so tests can see `Uri` -> `Url` normalisation instead of it being
/// hidden by starting from an already-normalised `Url`.
pub(super) fn insert_file(state: &mut WorldState, wire: &str, contents: &str) -> Uri {
    let uri: Uri = wire.parse().unwrap();
    let url = uri.to_url().unwrap();
    let file = state
        .db
        .upsert_editor(FilePath::from_url(&url), contents.to_string());
    state.insert_open_file(uri.clone(), FilePath::from_url(&url), file, None);
    uri
}

pub(super) fn range(start: (u32, u32), end: (u32, u32)) -> lsp_types::Range {
    lsp_types::Range {
        start: lsp_types::Position::new(start.0, start.1),
        end: lsp_types::Position::new(end.0, end.1),
    }
}
