use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::UNIX_EPOCH;

use oak_db::OakDatabase;

use super::source_handler::Gate;
use super::utils::test_client;
use super::utils::world_with_source_fetching;
use super::utils::write_sources;
use super::utils::DescriptionWriter;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::sidecar::PackageFacts;
use crate::lsp::sidecar::SidecarHandler;
use crate::lsp::sidecar::SidecarPackage;
use crate::lsp::sidecar::SidecarRequest;
use crate::lsp::sidecar::SidecarResponse;
use crate::lsp::sidecar::SidecarScheduler;
use crate::lsp::sources::SourceScheduler;
use crate::panic;

/// Supplies canned primitive stubs and lazy data names, and records requests.
/// An optional [`Gate`] holds each response so tests can request completions
/// or change the library while a batch is running.
///
/// Lazy data is keyed by package name, so tests can configure packages that
/// are installed after startup and have no `Package` yet.
pub(super) struct TestSidecarHandler {
    /// Keeps the stub files on disk until the handler is dropped.
    directory: tempfile::TempDir,
    stubs: Vec<(&'static str, &'static str)>,
    /// Behind a `Mutex` so a test can answer differently after an upgrade.
    lazy_data: Mutex<HashMap<String, Vec<String>>>,
    /// Package names for which the handler returns `None`, simulating a
    /// failed lazy data read.
    failing: HashSet<String>,
    /// Omit `primitive_stubs` from the response for `base`, simulating a
    /// failed stub generation.
    fails_stubs: bool,
    panics: bool,
    gate: Option<Gate>,
    calls: Mutex<Vec<SidecarRequest>>,
}

impl TestSidecarHandler {
    pub(super) fn new(stubs: Vec<(&'static str, &'static str)>) -> Self {
        Self {
            directory: tempfile::tempdir().unwrap(),
            stubs,
            lazy_data: Mutex::new(HashMap::new()),
            failing: HashSet::new(),
            fails_stubs: false,
            panics: false,
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

    /// Panics in [`Self::handle()`] after recording the request.
    pub(super) fn panicking() -> Self {
        Self {
            panics: true,
            ..Self::new(Vec::new())
        }
    }

    /// Answer `names` for package `name` when the request passes its
    /// `Rdata.rds`.
    pub(super) fn with_lazy_data(self, name: &str, names: &[&str]) -> Self {
        self.set_lazy_data(name, names);
        self
    }

    /// Supply `names` for package `name`. A gated request reads these names
    /// after the gate is released, so this can also affect an in-flight batch.
    pub(super) fn set_lazy_data(&self, name: &str, names: &[&str]) {
        let names = names.iter().map(|name| name.to_string()).collect();
        self.lazy_data
            .lock()
            .unwrap()
            .insert(name.to_string(), names);
    }

    /// Return `None` for package `name`'s lazy data request to simulate a
    /// failed read, rather than a successful read with no names.
    pub(super) fn with_failure(mut self, name: &str) -> Self {
        self.failing.insert(name.to_string());
        self
    }

    /// Omit `primitive_stubs` for `base`'s request to simulate failed stub
    /// generation.
    pub(super) fn with_stub_failure(mut self) -> Self {
        self.fails_stubs = true;
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

        if self.panics {
            panic!("Test panic in the sidecar job");
        }

        if let Some(gate) = &self.gate {
            gate.wait();
        }

        let packages = request
            .packages()
            .iter()
            .map(|package| {
                let primitive_stubs = (package.has_primitives() && !self.fails_stubs).then(|| {
                    let directory = self.directory.path().join("primitives");
                    write_sources(&directory, &self.stubs);
                    directory
                });
                PackageFacts {
                    package: package.package(),
                    primitive_stubs,
                    lazy_data: self.lazy_data(package),
                }
            })
            .collect();

        SidecarResponse { packages }
    }
}

impl TestSidecarHandler {
    fn lazy_data(&self, package: &SidecarPackage) -> Option<Vec<String>> {
        // The index is `<library>/<name>/data/Rdata.rds`.
        let name = package
            .lazy_data_file()?
            .parent()?
            .parent()?
            .file_name()?
            .to_str()?;

        if self.failing.contains(name) {
            return None;
        }

        let names = self.lazy_data.lock().unwrap();
        Some(names.get(name).cloned().unwrap_or_default())
    }
}

/// Disable source fetching so tests can isolate sidecar scheduling.
pub(super) fn sidecar_state(db: OakDatabase, sidecar: Arc<TestSidecarHandler>) -> GlobalState {
    let _boundary = panic::test_boundary();
    let mut scheduler = SourceScheduler::new(None);
    scheduler.config_arrived();
    GlobalState::from_parts(
        test_client(),
        world_with_source_fetching(db),
        LspState::new(tokio::sync::mpsc::unbounded_channel().0)
            .with_source_scheduler(scheduler)
            .with_sidecar_scheduler(SidecarScheduler::new(Some(sidecar))),
    )
}

/// Replace the installation of package `name` under `library`, adding an
/// empty `data/Rdata.rds` when `rdata` is set. Setting the `DESCRIPTION` mtime
/// to `mtime_seconds` after the Unix epoch lets tests control revisions
/// without depending on the filesystem's timestamp resolution.
pub(super) fn install_package(library: &Path, name: &str, mtime_seconds: u64, rdata: bool) {
    let dir = library.join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }

    let mut description = DescriptionWriter::new()
        .package(name)
        .version("0.0.0")
        .built("dummy");
    if name == "base" {
        description = description.field("Priority", "base");
    }
    description.write_installed(&dir);

    if rdata {
        std::fs::create_dir_all(dir.join("data")).unwrap();
        std::fs::write(dir.join("data").join("Rdata.rds"), "").unwrap();
    }

    std::fs::File::options()
        .write(true)
        .open(dir.join("DESCRIPTION"))
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(mtime_seconds))
        .unwrap();
}
