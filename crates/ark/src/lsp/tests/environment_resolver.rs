use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::anyhow;
use oak_scan::list_libraries;
use oak_scan::LibraryListing;

use super::source_handler::Gate;
use crate::lsp::environments::EnvironmentRequest;
use crate::lsp::environments::EnvironmentResolver;

/// A test [`EnvironmentResolver`] that serves canned behavior per environment
/// directory and records every call. Behaviors can be replaced between runs,
/// so a test can model a `.Rprofile` edit that changes the libraries.
pub(super) struct TestEnvironmentResolver {
    behavior: Mutex<HashMap<EnvironmentRequest, TestBehavior>>,
    calls: Mutex<Vec<EnvironmentRequest>>,
}

#[derive(Clone)]
pub(super) enum TestBehavior {
    /// List packages from disk for these paths without starting R.
    Success(Vec<PathBuf>),
    Failure(&'static str),
    Panic,
    /// Wait on a [`Gate`] before listing packages, so the test can send events
    /// while resolution is running.
    Gated(Arc<Gate>, Vec<PathBuf>),
}

impl TestEnvironmentResolver {
    pub(super) fn new(
        behavior: impl IntoIterator<Item = (EnvironmentRequest, TestBehavior)>,
    ) -> Self {
        Self {
            behavior: Mutex::new(behavior.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Replace the behavior for `request`, taking effect on its next run.
    pub(super) fn set(&self, request: EnvironmentRequest, behavior: TestBehavior) {
        self.behavior.lock().unwrap().insert(request, behavior);
    }

    pub(super) fn calls(&self) -> Vec<EnvironmentRequest> {
        self.calls.lock().unwrap().clone()
    }

    pub(super) fn call_count(&self, request: &EnvironmentRequest) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| *call == request)
            .count()
    }
}

impl EnvironmentResolver for TestEnvironmentResolver {
    fn resolve(&self, request: &EnvironmentRequest) -> anyhow::Result<Vec<LibraryListing>> {
        self.calls.lock().unwrap().push(request.clone());

        // Release the map lock before waiting on the gate so the test can
        // call `set()` while this run is blocked.
        let behavior = self.behavior.lock().unwrap().get(request).cloned();
        match behavior {
            Some(TestBehavior::Success(paths)) => Ok(list_libraries(paths)),
            Some(TestBehavior::Failure(message)) => Err(anyhow!(message)),
            Some(TestBehavior::Panic) => panic!("Test panic in an environment resolver"),
            Some(TestBehavior::Gated(gate, paths)) => {
                gate.wait();
                Ok(list_libraries(paths))
            },
            None => panic!("Unknown test environment {request:?}"),
        }
    }
}
