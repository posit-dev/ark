//! Package facts computed by a separate R process, without using the user's session.
//!
//! Batches request facts for library packages whose [`Package::facts_revision()`]
//! differs from their current `DESCRIPTION` revision, unless a request failed
//! at that revision. Jobs need no network and run independently of source
//! fetches in [`crate::lsp::sources`], regardless of
//! `oak.sourceFetching.enabled` or the outcome of a source fetch.
//! For installed `base`, they supply primitive stubs as generated files in
//! [`Package::files()`]. For installed packages with lazy data, they supply
//! the object names that `pkg::` lists through [`Package::lazy_data()`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use oak_db::Db;
use oak_db::FileRevision;
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

#[derive(Debug, Clone, Default)]
pub(crate) struct SidecarRequest {
    packages: Vec<SidecarPackage>,
}

/// An installed package with at least one fact to compute.
#[derive(Debug, Clone)]
pub(crate) struct SidecarPackage {
    package: Package,

    /// The `DESCRIPTION` revision the facts are requested for.
    description_revision: FileRevision,

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
    /// or the batch read failed. An unreadable index gives `Some` with no names
    /// and counts as an answered lazy data request.
    pub(crate) lazy_data: Option<Vec<String>>,
}

#[derive(Debug)]
pub(crate) struct SidecarCompleted {
    /// Retained so [`SidecarScheduler::finish()`] can reject stale revisions
    /// and detect missing facts, even when there is no response.
    pub(crate) request: SidecarRequest,

    /// `None` when the job stopped before reporting a response.
    pub(crate) response: Option<SidecarResponse>,
}

/// Request state for a [`Package`] at one `DESCRIPTION` revision.
/// Ingested facts are recorded in the database as [`Package::facts_revision()`].
enum SidecarState {
    Pending,

    /// Missing requested facts prevent retries at the same `DESCRIPTION` revision.
    Failed,
}

pub(crate) struct SidecarScheduler {
    handler: Option<Arc<dyn SidecarHandler>>,

    /// Retains at most one revision per package. All [`SidecarState::Pending`]
    /// entries belong to the same batch, so any pending entry blocks another
    /// batch from starting.
    state: HashMap<Package, (FileRevision, SidecarState)>,
}

impl SidecarScheduler {
    /// With no `handler`, no batch ever runs.
    pub(crate) fn new(handler: Option<Arc<dyn SidecarHandler>>) -> Self {
        Self {
            handler,
            state: HashMap::new(),
        }
    }

    /// Submit a batch on `pool` for library packages not covered by
    /// [`Package::facts_revision()`] and not pending or failed at their current
    /// `DESCRIPTION` revision. Results arrive as [`Event::SidecarCompleted`]
    /// for [`Self::finish()`] to validate before ingestion.
    ///
    /// Includes packages shadowed by an earlier library in R's library search
    /// order. At most one batch runs at a time. Packages needing facts that
    /// appear or change during a batch wait for the next batch.
    ///
    /// Returns packages with no facts to request, even while a batch is in
    /// flight. The caller must record them as covered with empty facts so
    /// answers that depend on them can settle without waiting for R.
    /// With no handler, submits nothing and returns no packages.
    ///
    /// The job owns no database handle, so waiting for R cannot block a
    /// main-loop write that needs exclusive database access.
    #[must_use]
    pub(crate) fn schedule(
        &mut self,
        db: &dyn Db,
        pool: &IoPool,
        events_tx: &TokioUnboundedSender<Event>,
    ) -> Vec<Package> {
        let Some(handler) = &self.handler else {
            return Vec::new();
        };

        let in_flight = self.has_pending();

        let mut covered = Vec::new();
        let mut packages = Vec::new();

        for package in library_packages(db) {
            let description_revision = package.description_revision(db);

            if package.facts_revision(db) == Some(description_revision) {
                continue;
            }
            if self
                .state
                .get(&package)
                .is_some_and(|(revision, _)| *revision == description_revision)
            {
                continue;
            }

            let Some(sidecar_package) = SidecarPackage::from_package(db, package) else {
                covered.push(package);
                continue;
            };
            if in_flight {
                continue;
            }

            self.state
                .insert(package, (description_revision, SidecarState::Pending));
            packages.push(sidecar_package);
        }

        if packages.is_empty() {
            return covered;
        }

        let handler = Arc::clone(handler);
        let guard = SidecarFailureGuard {
            request: SidecarRequest { packages },
            events_tx: Some(events_tx.clone()),
        };

        pool.submit(move || {
            let response = handler.handle(&guard.request);
            guard.complete(response);
        });

        covered
    }

    /// Whether facts can still arrive and change an answer for `package`.
    /// Returns `false` if there is no handler, the package is outside library
    /// roots, or its current `DESCRIPTION` revision is covered by
    /// [`Package::facts_revision()`] or recorded as failed.
    ///
    /// Waiting for a batch counts too. A package with no facts to compute
    /// returns `true` until the caller records coverage from [`Self::schedule()`].
    pub(crate) fn may_change(&self, db: &dyn Db, package: Package) -> bool {
        if self.handler.is_none() || !package.is_installed(db) {
            return false;
        }

        let description_revision = package.description_revision(db);
        if package.facts_revision(db) == Some(description_revision) {
            return false;
        }

        !matches!(
            self.state.get(&package),
            Some((revision, SidecarState::Failed)) if *revision == description_revision
        )
    }

    /// Return complete facts for packages still installed at the requested
    /// `DESCRIPTION` revision. The caller must ingest them and record
    /// [`Package::facts_revision()`] before scheduling again.
    ///
    /// Completions without a matching scheduler entry are ignored, so they
    /// cannot settle a request for a different revision. For matching entries,
    /// changed or removed packages are dropped and their entries cleared.
    /// They can be requested again if installed and still needing facts.
    ///
    /// If any requested fact is missing, record failure at the requested
    /// revision instead of returning partial facts. This also handles jobs
    /// that stop without a response.
    #[must_use]
    pub(crate) fn finish(&mut self, db: &dyn Db, completed: SidecarCompleted) -> Vec<PackageFacts> {
        let SidecarCompleted { request, response } = completed;

        let mut facts: HashMap<Package, PackageFacts> = response
            .into_iter()
            .flat_map(|response| response.packages)
            .map(|facts| (facts.package, facts))
            .collect();

        let mut ingested = Vec::new();

        for requested in request.packages {
            let package = requested.package;
            let description_revision = requested.description_revision;

            let Some((recorded, _)) = self.state.get(&package) else {
                continue;
            };
            if *recorded != description_revision {
                continue;
            }
            self.state.remove(&package);

            if package.description_revision(db) != description_revision {
                continue;
            }

            // Do not restore generated files after removal or eviction cleared
            // the package's files and coverage.
            if !package.is_installed(db) {
                continue;
            }

            match facts
                .remove(&package)
                .filter(|facts| requested.is_answered_by(facts))
            {
                Some(facts) => ingested.push(facts),
                None => {
                    self.state
                        .insert(package, (description_revision, SidecarState::Failed));
                },
            }
        }

        ingested
    }

    /// Whether a batch is in flight. Test pumps must check this separately
    /// from analysis-pool counters, which do not include the sidecar job.
    pub(crate) fn has_pending(&self) -> bool {
        self.state
            .values()
            .any(|(_, state)| matches!(state, SidecarState::Pending))
    }
}

/// Packages of every library root, in library search order.
fn library_packages(db: &dyn Db) -> impl Iterator<Item = Package> + '_ {
    db.library_roots()
        .roots(db)
        .iter()
        .flat_map(|root| root.packages(db).iter().copied())
        // A package that a deeper workspace root also holds is not installed
        // for analysis, and `SidecarScheduler::finish()` would drop its facts.
        // Selecting it anyway would submit it again after every completion.
        .filter(|package| package.is_installed(db))
}

/// Sends a completion without a response if the handler panics or the pool
/// drops the job without running it, so [`SidecarScheduler::finish()`] can
/// clear pending state. [`Self::complete()`] sends the response and disarms
/// the guard.
struct SidecarFailureGuard {
    request: SidecarRequest,
    events_tx: Option<TokioUnboundedSender<Event>>,
}

impl SidecarFailureGuard {
    fn complete(mut self, response: SidecarResponse) {
        self.send(Some(response));
    }

    fn send(&mut self, response: Option<SidecarResponse>) {
        let Some(events_tx) = self.events_tx.take() else {
            return;
        };
        let request = std::mem::take(&mut self.request);

        // `lsp::log_error!` uses the auxiliary channel, which can panic.
        // Use `log_err()` to avoid a second panic during `drop()` while the
        // handler is unwinding, which would abort the process.
        events_tx
            .send(Event::SidecarCompleted(SidecarCompleted {
                request,
                response,
            }))
            .log_err();
    }
}

impl Drop for SidecarFailureGuard {
    fn drop(&mut self) {
        self.send(None);
    }
}

impl SidecarRequest {
    /// Request facts for every library package that has any to compute.
    #[cfg(test)]
    pub(crate) fn from_db(db: &dyn Db) -> Self {
        let packages = library_packages(db)
            .filter_map(|package| SidecarPackage::from_package(db, package))
            .collect();

        Self { packages }
    }

    #[cfg(test)]
    pub(crate) fn packages(&self) -> &[SidecarPackage] {
        &self.packages
    }
}

impl SidecarPackage {
    /// Request primitive stubs for `base` when its cache-key fields are
    /// available, and lazy data names when `data/Rdata.rds` exists.
    /// Returns `None` if neither can be requested.
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
            description_revision: package.description_revision(db),
            base,
            lazy_data,
        })
    }

    fn is_answered_by(&self, facts: &PackageFacts) -> bool {
        (self.base.is_none() || facts.primitive_stubs.is_some()) &&
            (self.lazy_data.is_none() || facts.lazy_data.is_some())
    }

    #[cfg(test)]
    pub(crate) fn package(&self) -> Package {
        self.package
    }

    #[cfg(test)]
    pub(crate) fn description_revision(&self) -> FileRevision {
        self.description_revision
    }

    #[cfg(test)]
    pub(crate) fn has_primitives(&self) -> bool {
        self.base.is_some()
    }

    #[cfg(test)]
    pub(crate) fn has_lazy_data(&self) -> bool {
        self.lazy_data.is_some()
    }

    #[cfg(test)]
    pub(crate) fn lazy_data_file(&self) -> Option<&std::path::Path> {
        self.lazy_data.as_deref()
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
