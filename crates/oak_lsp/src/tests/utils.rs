mod description_writer;
pub mod events;
mod namespace_writer;

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use aether_path::FilePath;
pub use description_writer::DescriptionWriter;
pub use events::completion;
pub use events::did_change_configuration;
pub use events::did_change_watched_file;
pub use events::initialize;
pub use events::initialize_with_options;
pub use events::initialize_without_configuration;
pub use namespace_writer::NamespaceWriter;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use tower_lsp_server::ls_types as lsp_types;
use tower_lsp_server::ls_types::Uri;

use crate::environments::EnvironmentScheduler;
use crate::environments::FixedEnvironmentResolver;
pub use crate::harness::editor::client::test_client;
pub use crate::harness::editor::client::TestClient;
pub use crate::harness::editor::notifications::did_change;
pub use crate::harness::editor::notifications::did_change_workspace_folders;
pub use crate::harness::editor::notifications::did_open;
pub use crate::harness::editor::notifications::initialized;
use crate::sources::SourceHandler;
use crate::sources::SourceScheduler;
use crate::state::WorldState;
use crate::traits::url::UriExt;

/// Find R on the `PATH`
///
/// On Windows, `which` (from Git) returns POSIX paths that `Command::new()` can't resolve.
/// Use `where` which returns native paths.
pub fn find_r() -> PathBuf {
    let output = std::process::Command::new(if cfg!(windows) { "where" } else { "which" })
        .arg("R")
        .output()
        .unwrap_or_else(|err| panic!("Failed to find R: {err}"));
    assert!(output.status.success());

    // `where` on Windows can return multiple matches, take the first
    PathBuf::from(
        String::from_utf8(output.stdout)
            .expect("Non-UTF8 R path")
            .trim()
            .lines()
            .next()
            .expect("R should exist"),
    )
}

pub fn write_sources(dir: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(dir).unwrap();
    for (basename, contents) in files {
        std::fs::write(dir.join(basename), contents).unwrap();
    }
}

/// Resolve every environment to `libraries` without starting R. Pass the
/// database's existing library paths to preserve them through `initialize`.
pub fn fixed_environments(libraries: &[&Path]) -> EnvironmentScheduler {
    let paths = libraries.iter().map(|path| path.to_path_buf()).collect();
    EnvironmentScheduler::new(Some(Arc::new(FixedEnvironmentResolver::new(paths))))
}

/// Create a [`SourceScheduler`] with its startup configuration gate open.
pub fn source_scheduler_for_test(handler: Arc<dyn SourceHandler>) -> SourceScheduler {
    let mut scheduler = SourceScheduler::new(Some(handler));
    scheduler.config_arrived();
    scheduler
}

/// Keep source fetching enabled by default regardless of
/// `OAK_SOURCE_FETCHING_ENABLED`, without changing the process environment.
/// Tests can disable fetching through [`TestClient`], including on later pulls.
pub fn world_with_source_fetching(db: OakDatabase) -> WorldState {
    WorldState::with_default_source_fetching(db)
}

pub fn make_state(wire: &str, contents: &str) -> (WorldState, Uri) {
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
pub fn insert_file(state: &mut WorldState, wire: &str, contents: &str) -> Uri {
    let uri: Uri = wire.parse().unwrap();
    let url = uri.to_url().unwrap();
    let file = state
        .db_mut()
        .upsert_editor(FilePath::from_url(&url), contents.to_string());
    state.insert_open_file(uri.clone(), FilePath::from_url(&url), file, None);
    uri
}

pub fn range(start: (u32, u32), end: (u32, u32)) -> lsp_types::Range {
    lsp_types::Range {
        start: lsp_types::Position::new(start.0, start.1),
        end: lsp_types::Position::new(end.0, end.1),
    }
}

/// Create a URI from a file name.
/// File name is joined to the temp directory in a cross-platform way.
/// Returns a `Url`. Does not create the file.
#[cfg(test)]
pub(crate) fn test_path(file_name: &str) -> url::Url {
    let path = std::env::temp_dir().join(file_name);
    url::Url::from_file_path(&path).unwrap()
}
