//! Shared analysis setup for LSP tests and diagnostics benchmarks.
//!
//! [`LspHarness`] prepares analysis state without starting background work. Use
//! [`LspHarness::snapshot()`] and [`LspHarness::diagnose()`] for an isolated
//! diagnostics pass, or consume it with [`LspHarness::start()`] to exercise the
//! production handlers and schedulers through [`LspSession`].

pub mod client;
pub(crate) mod events;
pub mod session;

use aether_path::AbsPathBuf;
use aether_path::FilePath;
use oak_db::OakDatabase;
use oak_scan::DbScan;
pub use session::LspSession;
use tower_lsp_server::ls_types::Diagnostic;
use tower_lsp_server::ls_types::Uri;

use crate::lsp::analysis::is_testthat_path;
pub use crate::lsp::analysis::PoolMetrics;
use crate::lsp::analysis::WorldStateSnapshot;
use crate::lsp::diagnostics::generate_diagnostics;
use crate::lsp::state::Workspace;
use crate::lsp::state::WorldState;
use crate::lsp::traits::url::UriExt;

/// Owns the analysis inputs a diagnostics pass reads, with no executor
/// attached.
pub struct LspHarness {
    state: WorldState,
}

/// Keeps a [`WorldStateSnapshot`] private so snapshot creation remains
/// separately measurable from diagnostics.
pub struct HarnessSnapshot(WorldStateSnapshot);

impl LspHarness {
    pub fn new(db: OakDatabase) -> Self {
        Self {
            state: WorldState::new(db),
        }
    }

    /// Ignore `OAK_SOURCE_FETCHING_ENABLED` so editor configuration controls
    /// source fetching.
    #[cfg(test)]
    pub(crate) fn with_default_source_fetching(db: OakDatabase) -> Self {
        unsafe { std::env::remove_var(crate::lsp::config::OAK_SOURCE_FETCHING_ENABLED_ENV_VAR) };
        Self::new(db)
    }

    pub fn set_workspace_folders(&mut self, folders: Vec<AbsPathBuf>) {
        self.state.workspace = Workspace { folders };
    }

    pub fn set_installed_packages(&mut self, packages: Vec<String>) {
        self.state.installed_packages = packages;
    }

    /// Accept explicit console scopes for resolving base symbols because
    /// obtaining real scopes requires an R session.
    pub fn set_console_scopes(&mut self, scopes: Vec<Vec<String>>) {
        self.state.console_scopes = scopes;
    }

    /// Register an editor buffer without scheduling the work that a `didOpen`
    /// notification would trigger.
    pub fn prepare_document(&mut self, path: &FilePath, contents: String) -> anyhow::Result<()> {
        let file = self.state.db_mut().upsert_editor(path.clone(), contents);
        let uri = self.state.wire_uri(file)?;
        self.state.insert_open_file(uri, path.clone(), file, None);
        Ok(())
    }

    /// Register an editor buffer from the wire URI so tests exercise `Uri` to
    /// `Url` normalization.
    pub fn prepare_document_at(&mut self, wire: &str, contents: &str, version: Option<i32>) -> Uri {
        prepare_document_at(&mut self.state, wire, contents, version)
    }

    pub fn close_document(&mut self, path: &FilePath) {
        self.state.open_files.remove(path);
        self.state.db_mut().close_editor(path);
    }

    pub fn source_text(&self, path: &FilePath) -> anyhow::Result<&str> {
        let file = self.state.open_file(path)?.file();
        Ok(file.source_text(self.state.db()).as_str())
    }

    pub fn snapshot(&self) -> HarnessSnapshot {
        HarnessSnapshot(self.state.snapshot())
    }

    /// Generate diagnostics synchronously, without the auxiliary channel used
    /// by pool-driven refreshes.
    pub fn diagnose(
        &self,
        path: &FilePath,
        snapshot: HarnessSnapshot,
    ) -> anyhow::Result<Vec<Diagnostic>> {
        let open_file = self.state.open_file(path)?;
        Ok(generate_diagnostics(
            open_file.file(),
            snapshot.0,
            is_testthat_path(path),
            open_file.wire_uri(),
        ))
    }
}

/// Register an editor buffer in both `state.open_files` and `state.db` so
/// handler tests see the same document through either lookup.
pub(crate) fn prepare_document_at(
    state: &mut WorldState,
    wire: &str,
    contents: &str,
    version: Option<i32>,
) -> Uri {
    let uri: Uri = wire.parse().unwrap();
    let url = uri.to_url().unwrap();
    let file = state
        .db_mut()
        .upsert_editor(FilePath::from_url(&url), contents.to_string());
    state.insert_open_file(uri.clone(), FilePath::from_url(&url), file, version);
    uri
}
