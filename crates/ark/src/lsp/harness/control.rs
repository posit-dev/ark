//! A diagnostics pass on the caller's thread, with no main loop, scheduler,
//! or analysis pool involved. Benchmarks use it as the compute-only control
//! for the end-to-end measurements driven through `LspSession`.

use aether_path::FilePath;
use tower_lsp_server::ls_types::Diagnostic;

use super::LspHarness;
use crate::lsp::analysis::is_testthat_path;
use crate::lsp::analysis::WorldStateSnapshot;
use crate::lsp::diagnostics::generate_diagnostics;

/// Keeps a [`WorldStateSnapshot`] private so snapshot creation remains
/// separately measurable from diagnostics.
pub struct HarnessSnapshot(WorldStateSnapshot);

impl LspHarness {
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
