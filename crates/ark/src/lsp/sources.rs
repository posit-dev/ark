use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use aether_path::FilePath;
use oak_db::Db;
use oak_db::Package;
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

#[derive(Debug, Clone)]
pub(crate) struct SourceRequest {
    name: String,
    version: String,
    library_path: PathBuf,
}

// TODO!: Remove when we have a production `SourceHandler`
#[cfg_attr(not(test), expect(dead_code))]
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
    // TODO!: Remove the `Option<>` when we implement a production `SourceHandler`
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
            library_path,
        })
    }

    // TODO!: Remove when we have a production `SourceHandler`
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    // TODO!: Remove when we have a production `SourceHandler`
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    // TODO!: Remove when we have a production `SourceHandler`
    #[cfg_attr(not(test), expect(dead_code))]
    pub(crate) fn library_path(&self) -> &Path {
        &self.library_path
    }
}
