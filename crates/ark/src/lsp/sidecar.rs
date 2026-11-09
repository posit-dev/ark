//! Package facts computed by a separate R process, without using the user's session.
//!
//! One job runs per LSP session, independently of the per-package source fetches
//! in [`crate::lsp::sources`]. It needs no network and is not gated by
//! `oak.sourceFetching.enabled` or the outcome of a source fetch.
//! For installed `base`, it supplies primitive stubs as generated files in
//! [`Package::files()`].

use std::path::PathBuf;
use std::sync::Arc;

use oak_db::Db;
use oak_db::Package;
use oak_db::RootKind;
use oak_sidecar::primitives::PrimitivesCache;
use stdext::result::ResultExt;

use crate::lsp::io_pool::IoPool;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::TokioUnboundedSender;

/// Compute package facts without owning a database handle.
///
/// Tests can supply facts without starting R or using the shared cache.
pub(crate) trait SidecarHandler: Send + Sync {
    fn handle(&self, request: &SidecarRequest) -> SidecarResponse;
}

/// Generates primitive stubs with the local R and caches them on disk so
/// [`Package::files()`] can refer to them as files.
pub(crate) struct OakSidecarHandler {
    primitives: PrimitivesCache,
}

impl OakSidecarHandler {
    pub(crate) fn new(r: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            primitives: PrimitivesCache::open(r)?,
        })
    }
}

impl SidecarHandler for OakSidecarHandler {
    fn handle(&self, request: &SidecarRequest) -> SidecarResponse {
        let packages = request
            .packages
            .iter()
            .map(|package| PackageFacts {
                package: package.package,
                primitive_stubs: self.primitive_stubs(package),
            })
            .collect();

        SidecarResponse { packages }
    }
}

impl OakSidecarHandler {
    /// Find cached primitive stubs for `base`'s version and `Built:`,
    /// generating them with the local R if needed. `None` for other packages.
    fn primitive_stubs(&self, package: &SidecarPackage) -> Option<PathBuf> {
        if package.name != "base" {
            return None;
        }
        if let Some(directory) = self.primitives.get(&package.version, &package.built) {
            return Some(directory);
        }
        self.primitives.insert(&package.version, &package.built)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SidecarRequest {
    packages: Vec<SidecarPackage>,
}

/// An installed package, with the `DESCRIPTION` fields that key its cached facts.
#[derive(Debug, Clone)]
pub(crate) struct SidecarPackage {
    package: Package,
    name: String,
    version: String,
    built: String,
}

#[derive(Debug)]
pub(crate) struct SidecarResponse {
    pub(crate) packages: Vec<PackageFacts>,
}

#[derive(Debug)]
pub(crate) struct PackageFacts {
    pub(crate) package: Package,

    /// Cached directory of primitive stubs. `None` for packages other than
    /// `base`, or when no stubs could be obtained.
    pub(crate) primitive_stubs: Option<PathBuf>,
}

enum SidecarState {
    /// The job starts at the end of the first main-loop tick, after the library
    /// scan in [`super::main_loop::GlobalState::new()`] has registered packages.
    NotStarted,

    /// The job is in flight and can change these packages.
    Running(Vec<Package>),

    /// The response has arrived, or there was nothing to compute.
    Finished,
}

pub(crate) struct SidecarScheduler {
    handler: Option<Arc<dyn SidecarHandler>>,
    state: SidecarState,
}

impl SidecarScheduler {
    /// With no `handler`, the job never runs.
    pub(crate) fn new(handler: Option<Arc<dyn SidecarHandler>>) -> Self {
        Self {
            handler,
            state: SidecarState::NotStarted,
        }
    }

    /// Submit at most one job per scheduler, returning its facts through
    /// [`Event::SidecarCompleted`].
    /// The job owns no database handle, so waiting for R cannot block a
    /// main-loop write that needs exclusive database access.
    pub(crate) fn start(
        &mut self,
        db: &dyn Db,
        pool: &IoPool,
        events_tx: &TokioUnboundedSender<Event>,
    ) {
        if !matches!(self.state, SidecarState::NotStarted) {
            return;
        }
        let Some(handler) = &self.handler else {
            return;
        };

        let request = SidecarRequest::from_db(db);
        if request.packages.is_empty() {
            self.state = SidecarState::Finished;
            return;
        }

        let packages = request
            .packages
            .iter()
            .map(|package| package.package)
            .collect();
        self.state = SidecarState::Running(packages);

        let handler = Arc::clone(handler);
        let tx = events_tx.clone();
        pool.submit(move || {
            let response = handler.handle(&request);
            tx.send(Event::SidecarCompleted(response)).log_err();
        });
    }

    /// Whether the running job can still change `package`. If the job panics,
    /// no response arrives to clear `Running`, so its completion lists remain
    /// incomplete and test pumps keep waiting for the job.
    pub(crate) fn may_change(&self, package: Package) -> bool {
        match &self.state {
            SidecarState::Running(packages) => packages.contains(&package),
            SidecarState::NotStarted | SidecarState::Finished => false,
        }
    }

    /// Record that the job's response arrived. The caller ingests its facts.
    pub(crate) fn finish(&mut self) {
        self.state = SidecarState::Finished;
    }

    /// Test pumps must check this separately from analysis-pool counters,
    /// which do not include the sidecar job.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn has_pending(&self) -> bool {
        matches!(self.state, SidecarState::Running(_))
    }
}

impl SidecarRequest {
    /// Select installed `base`, the only package this job supplies facts for.
    /// A missing version or `Built:` field is logged and excludes the package,
    /// because both fields are required for the primitive-stub cache key.
    fn from_db(db: &dyn Db) -> Self {
        let packages = ["base"]
            .into_iter()
            .filter_map(|name| db.package_by_name(name))
            .filter(|package| {
                db.root_by_package(*package)
                    .is_some_and(|root| root.kind(db) == RootKind::Library)
            })
            .filter_map(|package| SidecarPackage::from_package(db, package).log_err())
            .collect();

        Self { packages }
    }

    #[cfg(test)]
    pub(crate) fn packages(&self) -> &[SidecarPackage] {
        &self.packages
    }
}

impl SidecarPackage {
    fn from_package(db: &dyn Db, package: Package) -> anyhow::Result<Self> {
        let name = package.name(db).clone();

        let Some(version) = package.version(db).to_owned() else {
            return Err(anyhow::anyhow!(
                "Package {name} is missing a version to compute sidecar facts for."
            ));
        };

        let Some(built) = package.built(db) else {
            return Err(anyhow::anyhow!(
                "Package {name} is missing a `Built` field to compute sidecar facts for."
            ));
        };

        Ok(Self {
            package,
            name,
            version,
            built,
        })
    }

    #[cfg(test)]
    pub(crate) fn package(&self) -> Package {
        self.package
    }

    #[cfg(test)]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}
