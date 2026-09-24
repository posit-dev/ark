//! Serves package sources a fixture wrote before the session started.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::lsp::sources::SourceHandler;
use crate::lsp::sources::SourceRequest;
use crate::lsp::sources::SourceResponse;

/// Report prewritten sources as freshly fetched so ingestion follows the
/// download path without network access or fixture writes during the request.
pub(super) struct PackageSources {
    directories: HashMap<String, PathBuf>,
}

impl PackageSources {
    pub(super) fn new(directories: HashMap<String, PathBuf>) -> Self {
        Self { directories }
    }
}

impl SourceHandler for PackageSources {
    fn handle(&self, request: &SourceRequest) -> SourceResponse {
        match self.directories.get(request.name()) {
            Some(directory) => SourceResponse::fetched(directory.clone()),
            None => SourceResponse::Failure,
        }
    }
}
