use std::collections::HashMap;
use std::sync::Mutex;

use oak_db::Package;

use super::source_handler::Gate;
use super::utils::write_sources;
use crate::lsp::sidecar::PackageFacts;
use crate::lsp::sidecar::SidecarHandler;
use crate::lsp::sidecar::SidecarRequest;
use crate::lsp::sidecar::SidecarResponse;

/// Supplies canned primitive stubs and lazy data names, and records requests.
/// An optional [`Gate`] holds the response so tests can request completions
/// while the job is running.
pub(super) struct TestSidecarHandler {
    /// Keeps the stub files on disk until the handler is dropped.
    directory: tempfile::TempDir,
    stubs: Vec<(&'static str, &'static str)>,
    lazy_data: HashMap<Package, Vec<String>>,
    gate: Option<Gate>,
    calls: Mutex<Vec<SidecarRequest>>,
}

impl TestSidecarHandler {
    pub(super) fn new(stubs: Vec<(&'static str, &'static str)>) -> Self {
        Self {
            directory: tempfile::tempdir().unwrap(),
            stubs,
            lazy_data: HashMap::new(),
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

    /// Answer `names` for `package` when the request passes its `Rdata.rds`.
    pub(super) fn with_lazy_data(mut self, package: Package, names: &[&str]) -> Self {
        let names = names.iter().map(|name| name.to_string()).collect();
        self.lazy_data.insert(package, names);
        self
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
                let primitive_stubs = package.has_primitives().then(|| {
                    let directory = self.directory.path().join("primitives");
                    write_sources(&directory, &self.stubs);
                    directory
                });
                let lazy_data = package.has_lazy_data().then(|| {
                    self.lazy_data
                        .get(&package.package())
                        .cloned()
                        .unwrap_or_default()
                });
                PackageFacts {
                    package: package.package(),
                    primitive_stubs,
                    lazy_data,
                }
            })
            .collect();

        SidecarResponse { packages }
    }
}
