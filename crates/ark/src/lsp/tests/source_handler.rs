use std::collections::HashMap;
use std::sync::Mutex;

use super::utils::write_sources;
use crate::lsp::sources::SourceHandler;
use crate::lsp::sources::SourceRequest;
use crate::lsp::sources::SourceResponse;

/// A test [`SourceHandler`] that serves canned behavior per package name and records
/// every call, so tests can count the number of calls. The handler is shared (as the
/// `Arc<dyn SourceHandler>` the `SourceScheduler` holds and the clone the test keeps), so
/// `calls` is a plain `Mutex`.
pub(super) struct TestSourceHandler {
    /// Owns the cache directory that `Success` writes per-package sources into.
    sources: tempfile::TempDir,
    /// Per-package canned behavior.
    behavior: HashMap<String, TestBehavior>,
    /// Each request passed to `handle`, in call order.
    calls: Mutex<Vec<SourceRequest>>,
}

// Canned behavior to perform when a particular package is requested
pub(super) enum TestBehavior {
    /// Write these `(basename, contents)` files into the package's source dir
    /// and return `Success(dir)`.
    Success(Vec<(&'static str, &'static str)>),
    Failure,
}

impl TestSourceHandler {
    pub(super) fn new(behavior: HashMap<String, TestBehavior>) -> Self {
        Self {
            sources: tempfile::tempdir().unwrap(),
            behavior,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// The requests passed to `handle`, in call order, for the test to assert on.
    pub(super) fn calls(&self) -> &Mutex<Vec<SourceRequest>> {
        &self.calls
    }
}

impl SourceHandler for TestSourceHandler {
    fn handle(&self, request: &SourceRequest) -> SourceResponse {
        self.calls.lock().unwrap().push(request.clone());

        match self.behavior.get(request.name()) {
            Some(TestBehavior::Success(files)) => {
                let dir = self.sources.path().join(request.name());
                write_sources(&dir, files);
                SourceResponse::Success(dir)
            },
            Some(TestBehavior::Failure) => SourceResponse::Failure,
            None => panic!("Unknown test package {}", request.name()),
        }
    }
}
