use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::FileRevision;
use oak_db::Package;
use oak_db::Priority;
use oak_source::SourceCache;
use oak_srcref::SrcrefCache;
use stdext::env_flag;
use stdext::is_ci;
use stdext::result::ResultExt;
use tokio::sync::watch;

use crate::config::OakConfig;
use crate::config::OAK_SOURCE_FETCHING_ENABLED_ENV_VAR;
use crate::handlers::Handlers;
use crate::io_pool::IoPool;
use crate::main_loop::Event;
use crate::main_loop::TokioUnboundedSender;

/// Handle source requests to provide R source files for an installed package
///
/// Implemented as a trait so we can have a production level [OakSourceHandler]
/// and a test level `TestSourceHandler`.
pub trait SourceHandler: Send + Sync {
    fn handle(&self, request: &SourceRequest) -> SourceResponse;
}

/// The production [`SourceHandler`]
///
/// Recovers a package's R sources in the following order:
/// - Base packages come from a downloaded base R source archive.
/// - CRAN packages come from:
///   - A local `srcref`, if available
///   - A downloaded CRAN package tarball
pub(crate) struct OakSourceHandler {
    srcref: SrcrefCache,
    source: SourceCache,
}

impl OakSourceHandler {
    /// Build the handler, opening every cache against the shared on disk cache
    pub(crate) fn new(r: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            srcref: SrcrefCache::open(r)?,
            source: SourceCache::open()?,
        })
    }

    /// Build the handler with every cache rooted under `root`, so tests don't touch the
    /// real on disk cache
    #[cfg(test)]
    pub(crate) fn new_in(root: &Path, r: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            srcref: SrcrefCache::open_in(root.join("srcref"), r)?,
            source: SourceCache::open_in(root.join("source"))?,
        })
    }
}

impl SourceHandler for OakSourceHandler {
    fn handle(&self, request: &SourceRequest) -> SourceResponse {
        let name = request.name();
        let version = request.version();

        // Base packages are served from a downloaded base R source archive, keyed by R
        // version
        if matches!(request.priority(), Some(Priority::Base)) {
            if let Some(root) = self.source.get_r(version) {
                return SourceResponse::cached(r_dir_for_base(&root, name));
            }
            if let Some(root) = self.source.insert_r(version) {
                return SourceResponse::fetched(r_dir_for_base(&root, name));
            }
            return SourceResponse::Failure;
        }

        // Try to "get" from all sources before doing an expensive "insert"
        if let Some(dir) = self.srcref.get(name, version, request.built()) {
            return SourceResponse::cached(dir);
        }
        if let Some(root) = self.source.get_cran(name, version) {
            return SourceResponse::cached(r_dir_for_cran(&root));
        }

        // Prefer `srcref` to CRAN download since it doesn't require internet and would
        // be an exact match to the installed package
        if let Some(dir) =
            self.srcref
                .insert(name, version, request.built(), request.library_path())
        {
            return SourceResponse::fetched(dir);
        }
        if let Some(root) = self.source.insert_cran(name, version) {
            return SourceResponse::fetched(r_dir_for_cran(&root));
        }

        SourceResponse::Failure
    }
}

/// Whether source fetching is disabled due to to CI env.
pub(crate) fn source_fetching_disabled_by_ci() -> bool {
    is_ci() && !env_flag(OAK_SOURCE_FETCHING_ENABLED_ENV_VAR)
}

/// Find `R/` from a CRAN package tarball
fn r_dir_for_cran(root: &Path) -> PathBuf {
    root.join("R")
}

/// Find `R/` from a base package tree, laid out as `{name}/R`
fn r_dir_for_base(root: &Path, name: &str) -> PathBuf {
    root.join(name).join("R")
}

#[derive(Debug, Clone)]
pub struct SourceRequest {
    name: String,
    version: String,
    built: String,
    priority: Option<Priority>,
    library_path: PathBuf,
}

#[derive(Debug)]
pub enum SourceResponse {
    Success {
        directory: PathBuf,
        origin: SourceOrigin,
    },
    Failure,

    /// Fetching was turned off while the job waited in the pool's queue, so no
    /// handler ran. Never returned by a [`SourceHandler`].
    Skipped,
}

/// Whether a successful response used cached sources or recovered them.
#[derive(Debug)]
pub enum SourceOrigin {
    /// An existing source-cache entry, with no network request.
    Cached,

    /// Recovered from a local `srcref` or downloaded source archive.
    Fetched,
}

impl SourceResponse {
    pub(crate) fn cached(directory: PathBuf) -> Self {
        Self::Success {
            directory,
            origin: SourceOrigin::Cached,
        }
    }

    pub(crate) fn fetched(directory: PathBuf) -> Self {
        Self::Success {
            directory,
            origin: SourceOrigin::Fetched,
        }
    }
}

#[derive(Debug)]
pub struct SourceCompleted {
    pub(crate) package: Package,

    /// The request's `DESCRIPTION` revision lets [`SourceScheduler::finish()`]
    /// reject a completion after a request for a newer revision is recorded.
    pub(crate) description_revision: FileRevision,

    pub(crate) response: SourceResponse,
}

/// State of a particular [Package]'s [SourceRequest] at one `DESCRIPTION`
/// revision. Fetched sources are recorded in the db as
/// [`Package::sources_revision()`], not here.
enum SourceState {
    /// The [SourceRequest] is in flight
    Pending,

    /// A failed fetch or an invalid request prevents retries for the same
    /// `DESCRIPTION` revision.
    Failed,
}

pub struct SourceScheduler {
    handler: Option<Arc<dyn SourceHandler>>,

    /// One entry per [`Package`], tagged with the `DESCRIPTION` revision it was
    /// recorded for. Replacing the entry on a new revision avoids accumulating
    /// state for older revisions.
    state: HashMap<Package, (FileRevision, SourceState)>,

    /// Blocks initial fetches until client configuration is resolved.
    awaiting_config: bool,

    /// Read by queued jobs to check for skip updates before starting.
    enabled_tx: watch::Sender<bool>,
}

impl SourceScheduler {
    pub fn new(handler: Option<Arc<dyn SourceHandler>>) -> Self {
        Self {
            handler,
            state: HashMap::new(),
            awaiting_config: true,
            enabled_tx: watch::Sender::new(OakConfig::default().source_fetching_enabled),
        }
    }

    /// Release fetches after attempting to load client configuration. While
    /// blocked, [`Self::schedule()`] leaves packages unrecorded so it can queue
    /// them later.
    pub fn config_arrived(&mut self) {
        self.awaiting_config = false;
    }

    /// Whether a handler was built at all. `false` means fetching is off for a
    /// reason that predates any setting, such as running on CI or a missing R.
    pub(crate) fn has_handler(&self) -> bool {
        self.handler.is_some()
    }

    /// On `pool`, run a fetch for each package that has no sources for its
    /// current `DESCRIPTION` revision and no fetch in flight or failed at that
    /// revision, shipping each [`SourceResponse`] back to the main loop as
    /// [`Event::SourceCompleted`]. [`Self::finish()`] settles the request and
    /// returns the source directory for the main loop to ingest.
    ///
    /// The job owns no db handle. A download can't be interrupted by a Salsa
    /// cancellation, so a handle in `pool`'s queue would hold up the next
    /// main-loop write for the whole fetch.
    pub(crate) fn schedule<H: Handlers>(
        &mut self,
        db: &dyn Db,
        config: &OakConfig,
        pool: &IoPool,
        events_tx: &TokioUnboundedSender<Event<H>>,
    ) {
        // Publish before returning to inform queued jobs of new setting value.
        self.enabled_tx.send_replace(config.source_fetching_enabled);

        // Wait for client configuration so `oak.sourceFetching.enabled = false`
        // may prevent startup fetches.
        if self.awaiting_config {
            return;
        }

        // Check every time so re-enabling source fetching queues packages left
        // unhandled while disabled.
        if !config.source_fetching_enabled {
            return;
        }

        let Some(handler) = &self.handler else {
            return;
        };

        for package in oak_db::all_package_dependencies(db) {
            let package = *package;
            let description_revision = package.description_revision(db);

            if package.sources_revision(db) == Some(description_revision) {
                continue;
            }
            if self
                .state
                .get(&package)
                .is_some_and(|(revision, _)| *revision == description_revision)
            {
                // In flight or failed at this `DESCRIPTION` revision
                continue;
            }

            let Some(request) = SourceRequest::from_package(db, &package).log_err() else {
                // Go straight to `Failed` if we can't generate the source request,
                // something is structurally wrong
                self.state
                    .insert(package, (description_revision, SourceState::Failed));
                continue;
            };

            let handler = Arc::clone(handler);
            let enabled_rx = self.enabled_tx.subscribe();

            // An older fetch may still be in flight. Replacing its recorded
            // revision makes `finish()` discard its completion without cancelling it.
            self.state
                .insert(package, (description_revision, SourceState::Pending));

            let on_abort = Event::SourceCompleted(SourceCompleted {
                package,
                description_revision,
                response: SourceResponse::Failure,
            });

            pool.submit_reporting(events_tx, on_abort, move || {
                // Copy the value before fetching so the `borrow()` guard does
                // not span `handle()`.
                let enabled = *enabled_rx.borrow();

                let response = if enabled {
                    handler.handle(&request)
                } else {
                    SourceResponse::Skipped
                };

                Event::SourceCompleted(SourceCompleted {
                    package,
                    description_revision,
                    response,
                })
            });
        }
    }

    /// Whether `package`'s sources may still arrive, so that an answer computed
    /// without them can change. True unless fetching is off, there is no
    /// handler, or the request for the current `DESCRIPTION` revision settled:
    /// its sources were ingested ([`Package::sources_revision()`]) or it
    /// failed.
    ///
    /// A package not requested yet at its current revision counts as pending:
    /// if a file uses it, the next tick-end scheduling queues it. A fetch job
    /// that panicked or never ran completes as [`SourceResponse::Failure`], so
    /// it settles like any other failure.
    pub(crate) fn sources_may_arrive(
        &self,
        db: &dyn Db,
        config: &OakConfig,
        package: Package,
    ) -> bool {
        if !config.source_fetching_enabled || self.handler.is_none() {
            return false;
        }

        let description_revision = package.description_revision(db);
        if package.sources_revision(db) == Some(description_revision) {
            return false;
        }

        !matches!(
            self.state.get(&package),
            Some((revision, SourceState::Failed)) if *revision == description_revision
        )
    }

    /// Settle a completion's request, returning successful sources for
    /// ingestion only if its `description_revision` is still the package's
    /// current revision in `db`. Ingesting them records that revision as
    /// [`Package::sources_revision()`], which stops further requests.
    ///
    /// A missing entry or a mismatch with the recorded revision returns `None`
    /// without changing state, so an older completion cannot settle a newer
    /// request.
    ///
    /// The recorded revision alone is not enough: a library rescan can apply an
    /// upgrade before `schedule()` records the new revision, because
    /// `refresh_if_due()` defers scheduling while events are queued, and
    /// `schedule()` records nothing while fetching is disabled. Such a
    /// completion is dropped and its entry removed, so the next `schedule()`
    /// requests the current revision.
    #[must_use]
    pub(crate) fn finish(
        &mut self,
        db: &dyn Db,
        package: Package,
        description_revision: FileRevision,
        response: SourceResponse,
    ) -> Option<PathBuf> {
        let (recorded, _) = self.state.get(&package)?;
        if *recorded != description_revision {
            return None;
        }
        self.state.remove(&package);

        if package.description_revision(db) != description_revision {
            return None;
        }

        match response {
            SourceResponse::Success { directory, .. } => Some(directory),
            SourceResponse::Failure => {
                self.state
                    .insert(package, (description_revision, SourceState::Failed));
                None
            },
            // Without an entry or a `sources_revision`, turning fetching back
            // on queues the package again.
            SourceResponse::Skipped => None,
        }
    }

    /// Source requests are tracked outside the analysis pool counters, so
    /// settlement must check them separately.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn has_pending(&self) -> bool {
        self.state
            .values()
            .any(|(_, state)| matches!(state, SourceState::Pending))
    }
}

impl SourceRequest {
    fn from_package(db: &dyn Db, package: &Package) -> anyhow::Result<Self> {
        let name = package.name(db).clone();

        let Some(version) = package.version(db).to_owned() else {
            return Err(anyhow::anyhow!(
                "Package {name} is missing a version to provide sources for."
            ));
        };

        let Some(built) = package.built(db).to_owned() else {
            // Only ever runs on installed packages, which always carry a `Built:` field
            return Err(anyhow::anyhow!(
                "Package {name} is missing a `Built` field to provide sources for."
            ));
        };

        let priority = package.priority(db).clone();

        let library_path = match package.description_path(db) {
            FilePath::File(path) => {
                match path.as_path().as_std_path().parent().and_then(Path::parent) {
                    Some(library_path) => library_path.to_path_buf(),
                    None => {
                        return Err(anyhow::anyhow!(
                            "Package {name} does not have an associated library path."
                        ))
                    },
                }
            },
            FilePath::Virtual(uri) => {
                return Err(anyhow::anyhow!(
                    "Package {name} is unexpectedly a virtual uri {uri}."
                ))
            },
        };

        Ok(Self {
            name,
            version,
            built,
            priority,
            library_path,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    pub(crate) fn built(&self) -> &str {
        &self.built
    }

    pub(crate) fn priority(&self) -> Option<&Priority> {
        self.priority.as_ref()
    }

    pub(crate) fn library_path(&self) -> &Path {
        &self.library_path
    }
}
