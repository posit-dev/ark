use std::sync::Mutex;

use super::source_handler::Gate;
use super::utils::write_sources;
use crate::lsp::sidecar::PackageFacts;
use crate::lsp::sidecar::SidecarHandler;
use crate::lsp::sidecar::SidecarRequest;
use crate::lsp::sidecar::SidecarResponse;

/// Supplies canned primitive stubs for `base` and records requests.
/// An optional [`Gate`] holds the response so tests can request completions
/// while the job is running.
pub(super) struct TestSidecarHandler {
    /// Keeps the stub files on disk until the handler is dropped.
    directory: tempfile::TempDir,
    stubs: Vec<(&'static str, &'static str)>,
    gate: Option<Gate>,
    calls: Mutex<Vec<SidecarRequest>>,
}

impl TestSidecarHandler {
    pub(super) fn new(stubs: Vec<(&'static str, &'static str)>) -> Self {
        Self {
            directory: tempfile::tempdir().unwrap(),
            stubs,
            gate: None,
            calls: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn gated(gate: Gate, stubs: Vec<(&'static str, &'static str)>) -> Self {
        Self {
            gate: Some(gate),
            ..Self::new(stubs)
        }
    }

    /// Requests passed to `handle()`, in call order.
    pub(super) fn calls(&self) -> &Mutex<Vec<SidecarRequest>> {
        &self.calls
    }
}

impl SidecarHandler for TestSidecarHandler {
    fn handle(&self, request: &SidecarRequest) -> SidecarResponse {
        self.calls.lock().unwrap().push(request.clone());

        if let Some(gate) = &self.gate {
            gate.wait();
        }

        let packages = request
            .packages()
            .iter()
            .map(|package| {
                let primitive_stubs = (package.name() == "base").then(|| {
                    let directory = self.directory.path().join("primitives");
                    write_sources(&directory, &self.stubs);
                    directory
                });
                PackageFacts {
                    package: package.package(),
                    primitive_stubs,
                }
            })
            .collect();

        SidecarResponse { packages }
    }
}
