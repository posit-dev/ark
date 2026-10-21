//! Shared analysis setup for LSP tests and diagnostics benchmarks.
//!
//! [`LspHarness`] prepares analysis state without starting background work.
//! The prepared state can then be measured in one of two ways:
//!
//! - `control`: [`LspHarness::snapshot()`] and [`LspHarness::diagnose()`] run
//!   one diagnostics pass on the caller's thread, with no main loop. This
//!   isolates diagnostics compute.
//! - `session`: [`LspHarness::start()`] consumes the harness and returns an
//!   [`LspSession`] that drives the production main loop, schedulers, and
//!   analysis pool end to end.
//!
//! When an end-to-end measurement moves and the control doesn't, the change
//! came from scheduling or synchronisation rather than compute.
//!
//! `editor` simulates the client on the other end of a session.
//!
//! # Panic boundaries
//!
//! A panic boundary lets a caller handle failure without killing the R kernel.
//! Otherwise, Ark's panic hook aborts: a background thread dying could leave the
//! kernel running with broken channels or partially updated state. A recovering
//! boundary combines a thread-local marker that permits unwinding with a
//! `catch_unwind()` that handles the panic. The marker is necessary because the
//! hook runs before unwinding reaches the catch.
//!
//! Salsa queries can panic, for example on a query cycle. In debug integration
//! tests, database access asserts that a boundary is active, detecting unprotected
//! access without needing to provoke a panic. The harness has two cases:
//!
//! - Event dispatch uses `GlobalState::handle_event_in_boundary()`, shared with
//!   production, to catch panics from the handler and deferred diagnostics refresh.
//!   Both callers absorb Salsa cancellation. For other panics, the LSP shuts down
//!   the session; the harness resumes unwinding to fail the test.
//! - Setup and inspection, such as `prepare_document()` and `snapshot()`, access
//!   Salsa outside event dispatch and need a scoped `panic::test_boundary()` guard.
//!   It permits unwinding to the test or benchmark runner, which reports the
//!   failure to the user and exits with a nonzero status. The guard does not catch
//!   the panic; it restores the previous marker when dropped. Its state is
//!   thread-local, so Clippy rejects holding it across `.await`.
//!
//! Never extend a harness guard around event dispatch. The assertion checks for
//! *any* active boundary, so an enclosing guard would hide missing recovery
//! boundaries in the event code under test.

mod control;
pub(crate) mod editor;
mod session;

use aether_path::AbsPathBuf;
use aether_path::FilePath;
pub use control::HarnessSnapshot;
use oak_db::OakDatabase;
use oak_scan::DbScan;
pub use session::DefinitionAnswer;
pub use session::DefinitionRequest;
pub use session::DefinitionResult;
pub use session::LspSession;
pub use session::Publication;
pub use session::RequestAnswer;
pub use session::RequestHandle;

use crate::console::console_scopes;
pub use crate::lsp::analysis::DiagnosticsMetrics;
pub use crate::lsp::analysis::PoolMetrics;
pub use crate::lsp::analysis::MAX_ANALYSIS_THREADS_ENV_VAR;
use crate::lsp::state::Workspace;
use crate::lsp::state::WorldState;
use crate::panic;

/// Owns the analysis inputs a diagnostics pass reads, with no executor
/// attached.
pub struct LspHarness {
    state: WorldState,
}

impl LspHarness {
    pub fn new(db: OakDatabase) -> Self {
        Self {
            state: WorldState::new(db),
        }
    }

    /// Ignore `OAK_SOURCE_FETCHING_ENABLED` throughout the session so editor
    /// configuration controls source fetching. Other environment overrides
    /// still apply, and the process environment is unchanged.
    pub fn with_default_source_fetching(db: OakDatabase) -> Self {
        Self {
            state: WorldState::with_default_source_fetching(db),
        }
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
        let _boundary = panic::test_boundary();

        let file = self.state.db_mut().upsert_editor(path.clone(), contents);
        let uri = self.state.wire_uri(file)?;
        self.state.insert_open_file(uri, path.clone(), file, None);
        Ok(())
    }

    pub fn close_document(&mut self, path: &FilePath) {
        let _boundary = panic::test_boundary();

        self.state.open_files.remove(path);
        self.state.db_mut().close_editor(path);
    }

    pub fn source_text(&self, path: &FilePath) -> anyhow::Result<&str> {
        let _boundary = panic::test_boundary();

        let file = self.state.open_file(path)?.file();
        Ok(file.source_text(self.state.db()).as_str())
    }
}

/// Initialize R and return the `ReadConsole` scopes needed to resolve base
/// symbols in diagnostics benchmarks.
pub fn r_session_scopes() -> Vec<Vec<String>> {
    harp::fixtures::r_test_init();
    console_scopes()
}
