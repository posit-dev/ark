//! Package facts computed by a separate R process, without using the user's session.
//!
//! One job runs per LSP session, independently of the per-package source fetches
//! in [`crate::lsp::sources`]. It needs no network and is not gated by
//! `oak.sourceFetching.enabled` or the outcome of a source fetch.
//! For installed `base`, it supplies primitive stubs as generated files in
//! [`Package::files()`]. For installed packages with lazy data, it supplies
//! the object names that `pkg::` lists through [`Package::lazy_data()`].

use std::path::PathBuf;
use std::sync::Arc;

use oak_db::Db;
use oak_db::Package;
use oak_sidecar::lazy_data;
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

/// Uses the local R to compute package facts. Primitive stubs are cached on
/// disk for [`Package::files()`]. Lazy data indexes for all packages in the
/// request are read in a single R process.
pub(crate) struct OakSidecarHandler {
    r: PathBuf,
    primitives: PrimitivesCache,
}

impl OakSidecarHandler {
    pub(crate) fn new(r: PathBuf) -> anyhow::Result<Self> {
        let primitives = PrimitivesCache::open(r.clone())?;
        Ok(Self { r, primitives })
    }

    #[cfg(test)]
    pub(crate) fn with_primitives(r: PathBuf, primitives: PrimitivesCache) -> Self {
        Self { r, primitives }
    }
}

impl SidecarHandler for OakSidecarHandler {
    fn handle(&self, request: &SidecarRequest) -> SidecarResponse {
        // Results follow request order but omit packages without `Rdata.rds`.
        let mut lazy_data = self.lazy_data(request).into_iter();

        let packages = request
            .packages
            .iter()
            .map(|package| {
                let lazy_data = match package.lazy_data {
                    Some(_) => lazy_data.next().flatten(),
                    None => None,
                };
                PackageFacts {
                    package: package.package,
                    primitive_stubs: self.primitive_stubs(package),
                    lazy_data,
                }
            })
            .collect();

        SidecarResponse { packages }
    }
}

impl OakSidecarHandler {
    /// Find cached primitive stubs for `base`'s version and `Built:`,
    /// generating them with the local R if needed. `None` for other packages.
    fn primitive_stubs(&self, package: &SidecarPackage) -> Option<PathBuf> {
        let base = package.base.as_ref()?;
        if let Some(directory) = self.primitives.get(&base.version, &base.built) {
            return Some(directory);
        }
        self.primitives.insert(&base.version, &base.built)
    }

    /// Returns one entry per requested `Rdata.rds`, in request order.
    /// If [`lazy_data::read_object_names()`] returns an error, every entry is
    /// `None`. An unreadable individual index gives `Some` with no names.
    fn lazy_data(&self, request: &SidecarRequest) -> Vec<Option<Vec<String>>> {
        let files: Vec<PathBuf> = request
            .packages
            .iter()
            .filter_map(|package| package.lazy_data.clone())
            .collect();

        match lazy_data::read_object_names(&self.r, &files) {
            Ok(names) => names.into_iter().map(Some).collect(),
            Err(err) => {
                log::error!("Failed to read lazy data names: {err:?}");
                vec![None; files.len()]
            },
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct SidecarRequest {
    packages: Vec<SidecarPackage>,
}

/// An installed package with at least one fact to compute.
#[derive(Debug, Clone)]
pub(crate) struct SidecarPackage {
    package: Package,

    /// The build fields that key the primitive-stub cache. `Some` only for
    /// `base`.
    base: Option<BaseBuild>,

    /// The package's `data/Rdata.rds`, which lists its lazy data objects.
    /// `Some` only when the file exists.
    lazy_data: Option<PathBuf>,
}

/// `base`'s `DESCRIPTION` fields, which key its cached primitive stubs.
#[derive(Debug, Clone)]
struct BaseBuild {
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

    /// Lazy data object names, in any order. `None` if no index was requested
    /// or the batch read failed. An unreadable index gives `Some` with no names.
    pub(crate) lazy_data: Option<Vec<String>>,
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
    /// Selects `base` for primitive stubs and packages with `data/Rdata.rds`
    /// for lazy data names, across all library roots. This includes packages
    /// shadowed by an earlier library in the library search order.
    ///
    /// Checking for `Rdata.rds` before scheduling keeps packages with neither
    /// a lazy data index nor primitive stubs out of
    /// [`SidecarScheduler::may_change()`]. Only existing index files are passed
    /// to R for lazy data names.
    pub(crate) fn from_db(db: &dyn Db) -> Self {
        let packages = db
            .library_roots()
            .roots(db)
            .iter()
            .flat_map(|root| root.packages(db).iter())
            .filter_map(|package| SidecarPackage::from_package(db, *package))
            .collect();

        Self { packages }
    }

    #[cfg(test)]
    pub(crate) fn packages(&self) -> &[SidecarPackage] {
        &self.packages
    }
}

impl SidecarPackage {
    fn from_package(db: &dyn Db, package: Package) -> Option<Self> {
        let base = if package.name(db) == "base" {
            BaseBuild::from_package(db, package).log_err()
        } else {
            None
        };

        let lazy_data = package
            .description_path(db)
            .as_path()
            .and_then(|path| path.parent())
            .map(|directory| lazy_data::rdata_file(directory.as_std_path()))
            .filter(|file| file.is_file());

        if base.is_none() && lazy_data.is_none() {
            return None;
        }

        Some(Self {
            package,
            base,
            lazy_data,
        })
    }

    #[cfg(test)]
    pub(crate) fn package(&self) -> Package {
        self.package
    }

    #[cfg(test)]
    pub(crate) fn has_primitives(&self) -> bool {
        self.base.is_some()
    }

    #[cfg(test)]
    pub(crate) fn has_lazy_data(&self) -> bool {
        self.lazy_data.is_some()
    }
}

impl BaseBuild {
    /// A missing version or `Built:` field is an error, because both fields
    /// are required for the primitive-stub cache key.
    fn from_package(db: &dyn Db, package: Package) -> anyhow::Result<Self> {
        let name = package.name(db);

        let Some(version) = package.version(db).to_owned() else {
            return Err(anyhow::anyhow!(
                "Package {name} is missing a version to compute primitive stubs for."
            ));
        };

        let Some(built) = package.built(db) else {
            return Err(anyhow::anyhow!(
                "Package {name} is missing a `Built` field to compute primitive stubs for."
            ));
        };

        Ok(Self { version, built })
    }
}
