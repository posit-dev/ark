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
    Failed,
    Retry,
}

#[derive(Debug)]
pub(crate) struct SourceCompleted {
    pub(crate) package: Package,
    pub(crate) response: SourceResponse,
}

enum SourceState {
    Pending,
    Success,
    Failed,
    Retry,
}

pub(crate) struct SourceManager {
    // TODO!: Remove the `Option<>` when we implement a production `SourceHandler`
    handler: Option<Arc<dyn SourceHandler>>,
    state: HashMap<Package, SourceState>,
}

impl SourceManager {
    pub(crate) fn new(handler: Option<Arc<dyn SourceHandler>>) -> Self {
        Self {
            handler,
            state: HashMap::new(),
        }
    }

    pub(crate) fn dispatch(&mut self, db: &dyn Db, events_tx: &TokioUnboundedSender<Event>) {
        let Some(handler) = &self.handler else {
            return;
        };

        // For each package used by the workspace, request its sources if we have never
        // seen it before (or if it needs a retry)
        for package in oak_db::workspace_dependencies(db) {
            if !self.should_dispatch(package) {
                continue;
            }

            let package = *package;

            let Some(request) = SourceRequest::from_package(db, &package).log_err() else {
                // Never retry this package if we couldn't convert it to a source request
                self.state.insert(package, SourceState::Failed);
                continue;
            };

            let handler = Arc::clone(handler);
            let tx = events_tx.clone();

            // Set to `Pending` after all possible early exits
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

    fn should_dispatch(&self, package: &Package) -> bool {
        match self.state.get(package) {
            Some(state) => match state {
                SourceState::Pending => false,
                SourceState::Success => false,
                SourceState::Failed => false,
                SourceState::Retry => true,
            },
            None => true,
        }
    }

    #[must_use]
    pub(crate) fn finish(&mut self, package: Package, response: SourceResponse) -> Option<PathBuf> {
        let (next, directory) = match response {
            SourceResponse::Success(directory) => (SourceState::Success, Some(directory)),
            SourceResponse::Failed => (SourceState::Failed, None),
            SourceResponse::Retry => (SourceState::Retry, None),
        };
        self.state.insert(package, next);
        directory
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
