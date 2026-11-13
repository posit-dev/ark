//! A diagnostics pass on the caller's thread, with no main loop, scheduler,
//! or analysis pool involved. Benchmarks use it as the compute-only control
//! for the end-to-end measurements driven through `LspSession`.

use aether_path::FilePath;
use legacy_ide::diagnostics::generate_diagnostics;
use tower_lsp_server::ls_types::Diagnostic;

use super::LspHarness;
use crate::analysis::is_testthat_path;
use crate::analysis::WorldStateSnapshot;
use crate::panic;

/// Keeps a [`WorldStateSnapshot`] private so snapshot creation remains
/// separately measurable from diagnostics.
pub struct HarnessSnapshot(WorldStateSnapshot);

impl LspHarness {
    pub fn snapshot(&self) -> HarnessSnapshot {
        let _boundary = panic::test_boundary();
        HarnessSnapshot(self.state.snapshot())
    }

    /// Generate diagnostics synchronously, without the auxiliary channel used
    /// by pool-driven refreshes.
    pub fn diagnose(
        &self,
        path: &FilePath,
        snapshot: HarnessSnapshot,
    ) -> anyhow::Result<Vec<Diagnostic>> {
        let _boundary = panic::test_boundary();

        let open_file = self.state.open_file(path)?;
        let HarnessSnapshot(snapshot) = snapshot;
        Ok(generate_diagnostics(
            snapshot.db(),
            open_file.file(),
            open_file.wire_uri(),
            &snapshot.config.diagnostics,
            snapshot.config.position_encoding,
            &snapshot.session_scopes,
            is_testthat_path(path),
        ))
    }
}
