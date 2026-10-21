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

use crate::lsp::config::OakConfig;
use crate::lsp::config::OAK_SOURCE_FETCHING_ENABLED_ENV_VAR;
use crate::lsp::io_pool::IoPool;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::TokioUnboundedSender;

/// Handle source requests to provide R source files for an installed package
///
/// Implemented as a trait so we can have a production level [OakSourceHandler]
/// and a test level `TestSourceHandler`.
pub(crate) trait SourceHandler: Send + Sync {
    fn handle(&self, request: &SourceRequest) -> SourceResponse;
}

/// The production [`SourceHandler`]
///
/// Recovers a package's R sources in the following order:
/// - Base packages come from a downloaded base R source archive
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
pub(crate) struct SourceRequest {
    name: String,
    version: String,
    built: String,
    priority: Option<Priority>,
    library_path: PathBuf,
}

#[derive(Debug)]
pub(crate) enum SourceResponse {
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
pub(crate) enum SourceOrigin {
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
pub(crate) struct SourceCompleted {
    pub(crate) package: Package,

    /// The request's `DESCRIPTION` revision lets [`SourceScheduler::finish()`]
    /// reject a completion after a request for a newer revision is recorded.
    pub(crate) description_revision: FileRevision,

    pub(crate) response: SourceResponse,
}

/// State of a particular [Package]'s [SourceRequest]
///
/// A package is treated as unseen when it has no entry in `state` or the
/// entry's `DESCRIPTION` revision differs from its current revision.
enum SourceState {
    /// The [SourceRequest] is in flight
    Pending,

    /// Success, failure, or an invalid request prevents retries for the same
    /// `DESCRIPTION` revision.
    Finished,
}

pub(crate) struct SourceScheduler {
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
    pub(crate) fn new(handler: Option<Arc<dyn SourceHandler>>) -> Self {
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
    pub(crate) fn config_arrived(&mut self) {
        self.awaiting_config = false;
    }

    /// Whether a handler was built at all. `false` means fetching is off for a
    /// reason that predates any setting, such as running on CI or a missing R.
    pub(crate) fn has_handler(&self) -> bool {
        self.handler.is_some()
    }

    /// Run a fetch for each package we haven't seen before on `pool`, shipping
    /// each [`SourceResponse`] back to the main loop as
    /// [`Event::SourceCompleted`], where [`Self::finish`] then applies it.
    ///
    /// The job owns no db handle. A download can't be interrupted by a Salsa
    /// cancellation, so a handle in `pool`'s queue would hold up the next
    /// main-loop write for the whole fetch.
    pub(crate) fn schedule(
        &mut self,
        db: &dyn Db,
        config: &OakConfig,
        pool: &IoPool,
        events_tx: &TokioUnboundedSender<Event>,
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

            if self
                .state
                .get(&package)
                .is_some_and(|(revision, _)| *revision == description_revision)
            {
                // Already requested or finished at this `DESCRIPTION` revision
                continue;
            }

            let Some(request) = SourceRequest::from_package(db, &package).log_err() else {
                // Go straight to `Finished` if we can't generate the source request,
                // something is structurally wrong
                self.state
                    .insert(package, (description_revision, SourceState::Finished));
                continue;
            };

            let handler = Arc::clone(handler);
            let enabled_rx = self.enabled_tx.subscribe();
            let tx = events_tx.clone();

            // An older fetch may still be in flight. Replacing its recorded
            // revision makes `finish()` discard its completion without cancelling it.
            self.state
                .insert(package, (description_revision, SourceState::Pending));

            pool.submit(move || {
                // Copy the value before fetching so the `borrow()` guard does
                // not span `handle()`.
                let enabled = *enabled_rx.borrow();

                let response = if enabled {
                    handler.handle(&request)
                } else {
                    SourceResponse::Skipped
                };

                tx.send(Event::SourceCompleted(SourceCompleted {
                    package,
                    description_revision,
                    response,
                }))
                .log_err();
            });
        }
    }

    /// Apply a completion only if its `description_revision` matches both the
    /// package's recorded revision and its current revision in `db`, returning
    /// successful sources for ingestion.
    ///
    /// A missing entry or a mismatch with the recorded revision returns `None`
    /// without changing state, so an older completion cannot finish or remove a
    /// newer request.
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
        if package.description_revision(db) != description_revision {
            self.state.remove(&package);
            return None;
        }

        match response {
            SourceResponse::Success { directory, .. } => {
                self.state
                    .insert(package, (description_revision, SourceState::Finished));
                Some(directory)
            },
            SourceResponse::Failure => {
                self.state
                    .insert(package, (description_revision, SourceState::Finished));
                None
            },
            SourceResponse::Skipped => {
                // Forget the package, so turning fetching back on queues it again.
                self.state.remove(&package);
                None
            },
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

    pub(crate) fn name(&self) -> &str {
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
