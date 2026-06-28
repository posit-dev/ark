use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::Package;
use oak_db::Priority;
use oak_source::SourceCache;
use oak_srcref::SrcrefCache;
use stdext::result::ResultExt;

use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::TokioUnboundedSender;

/// Handle source requests to provide R source files for an installed package
///
/// Implementations live outside of oak, oak is only in charge of ingesting the
/// returned directory.
pub(crate) trait SourceHandler: Send + Sync {
    fn handle(&self, request: &SourceRequest) -> SourceResponse;
}

/// The production [`SourceHandler`]
///
/// Recovers a package's R sources in the following order:
/// - Base packages come from a downloaded CRAN's R source tarball
/// - CRAN packages come from:
///   - A local `srcref`, if available
///   - A downloaded CRAN package tarball
pub(crate) struct OakSourceHandler {
    srcref: SrcrefCache,
    source: SourceCache,
}

impl OakSourceHandler {
    /// Build the handler, opening both caches against the shared on disk cache
    pub(crate) fn new(r: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            srcref: SrcrefCache::new(r)?,
            source: SourceCache::new()?,
        })
    }

    /// Build the handler with both caches rooted under `root`, so tests don't touch the
    /// real on disk cache
    #[cfg(test)]
    pub(crate) fn new_in(root: &Path, r: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            srcref: SrcrefCache::new_in(root.join("srcref"), r)?,
            source: SourceCache::new_in(root.join("source"))?,
        })
    }
}

impl SourceHandler for OakSourceHandler {
    fn handle(&self, request: &SourceRequest) -> SourceResponse {
        let name = request.name();
        let version = request.version();

        // Base packages only exist in the R version source tarball, served from one
        // download shared across all of them
        if matches!(request.priority(), Some(Priority::Base)) {
            return self
                .source
                .get_r(version)
                .or_else(|| self.source.insert_r(version))
                .map(|root| SourceResponse::Success(r_dir_for_base(&root, name)))
                .unwrap_or(SourceResponse::Failure);
        }

        // Try to "get" from all sources before doing an expensive "insert"
        if let Some(dir) = self.srcref.get(name, version, request.built()) {
            return SourceResponse::Success(dir);
        }
        if let Some(root) = self.source.get_cran(name, version) {
            return SourceResponse::Success(r_dir_for_cran(&root));
        }

        // Prefer `srcref` to CRAN download since it doesn't require internet and would
        // be an exact match to the installed package
        if let Some(dir) =
            self.srcref
                .insert(name, version, request.built(), request.library_path())
        {
            return SourceResponse::Success(dir);
        }
        if let Some(root) = self.source.insert_cran(name, version) {
            return SourceResponse::Success(r_dir_for_cran(&root));
        }

        SourceResponse::Failure
    }
}

/// Find `R/` from a CRAN package tarball
fn r_dir_for_cran(root: &Path) -> PathBuf {
    root.join("R")
}

/// Find `R/` from a R source tarball for a base package
fn r_dir_for_base(root: &Path, name: &str) -> PathBuf {
    root.join("src").join("library").join(name).join("R")
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
    Success(PathBuf),
    Failure,
}

#[derive(Debug)]
pub(crate) struct SourceCompleted {
    pub(crate) package: Package,
    pub(crate) response: SourceResponse,
}

/// State of a particular [Package]'s [SourceRequest]
///
/// There is also a 3rd implied state of "we've never seen this package before" if it
/// isn't in the `state` hash map.
enum SourceState {
    /// The [SourceRequest] is in flight
    Pending,

    /// We have received a [SourceResponse]. Regardless of [SourceResponse::Success] or
    /// [SourceResponse::Failure], we mark the package as `Finished` so we never request
    /// it again.
    Finished,
}

pub(crate) struct SourceScheduler {
    handler: Option<Arc<dyn SourceHandler>>,
    state: HashMap<Package, SourceState>,
}

impl SourceScheduler {
    pub(crate) fn new(handler: Option<Arc<dyn SourceHandler>>) -> Self {
        Self {
            handler,
            state: HashMap::new(),
        }
    }

    pub(crate) fn schedule(&mut self, db: &dyn Db, events_tx: &TokioUnboundedSender<Event>) {
        let Some(handler) = &self.handler else {
            return;
        };

        // For each package used by the workspace, request its sources if we have never
        // seen it before
        for package in oak_db::workspace_dependencies(db) {
            if self.state.contains_key(package) {
                // If we've seen this package before, don't request sources again!
                continue;
            }

            let package = *package;

            let Some(request) = SourceRequest::from_package(db, &package).log_err() else {
                // Go straight to `Finished` if we can't generate the source request,
                // something is structurally wrong
                self.state.insert(package, SourceState::Finished);
                continue;
            };

            let handler = Arc::clone(handler);
            let tx = events_tx.clone();

            // Mark as `Pending` just before launching the tokio task
            self.state.insert(package, SourceState::Pending);

            crate::lsp::spawn_blocking(move || {
                let response = handler.handle(&request);

                tx.send(Event::SourceCompleted(SourceCompleted {
                    package,
                    response,
                }))
                .log_err();

                Ok(None)
            });
        }
    }

    #[must_use]
    pub(crate) fn finish(&mut self, package: Package, response: SourceResponse) -> Option<PathBuf> {
        self.state.insert(package, SourceState::Finished);
        match response {
            SourceResponse::Success(directory) => Some(directory),
            SourceResponse::Failure => None,
        }
    }

    /// Whether any source request is in flight. Allows tests to deterministically "wait"
    /// for pending source requests to finish.
    #[cfg(test)]
    pub(crate) fn has_pending(&self) -> bool {
        self.state
            .values()
            .any(|state| matches!(state, SourceState::Pending))
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
