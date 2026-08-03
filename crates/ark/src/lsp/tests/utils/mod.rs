mod client;
mod description_writer;
mod events;
mod namespace_writer;

use std::path::Path;
use std::sync::Arc;

use aether_path::FilePath;
pub(super) use client::test_client;
pub(super) use client::TestClient;
pub(super) use description_writer::DescriptionWriter;
pub(super) use events::did_change;
pub(super) use events::did_change_configuration;
pub(super) use events::did_change_workspace_folders;
pub(super) use events::did_open;
pub(super) use events::initialize;
pub(super) use events::initialize_without_configuration;
pub(super) use events::initialized;
pub(super) use namespace_writer::NamespaceWriter;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::ls_types::Uri;

use crate::lsp::config::OAK_SOURCE_FETCHING_ENABLED_ENV_VAR;
use crate::lsp::sources::SourceHandler;
use crate::lsp::sources::SourceScheduler;
use crate::lsp::state::WorldState;
use crate::lsp::traits::url::UriExt;

pub(super) fn write_sources(dir: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(dir).unwrap();
    for (basename, contents) in files {
        std::fs::write(dir.join(basename), contents).unwrap();
    }
}

/// Create a [`SourceScheduler`] with its startup configuration gate open.
pub(super) fn source_scheduler_for_test(handler: Arc<dyn SourceHandler>) -> SourceScheduler {
    let mut scheduler = SourceScheduler::new(Some(handler));
    scheduler.config_arrived();
    scheduler
}

/// Creates a [`WorldState`] with default source fetching by removing the ambient
/// `OAK_SOURCE_FETCHING_ENABLED`. Tests disable it through [`TestClient`].
pub(super) fn world_with_source_fetching(db: OakDatabase) -> WorldState {
    unsafe { std::env::remove_var(OAK_SOURCE_FETCHING_ENABLED_ENV_VAR) };
    WorldState::new(db)
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
