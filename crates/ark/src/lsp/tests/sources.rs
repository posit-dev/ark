//! Tests that drive the source request pipeline through the real [`GlobalState`]
//! event loop.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

use oak_db::Db;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use oak_semantic::library::Library;
use tower_lsp::lsp_types::DidChangeWorkspaceFoldersParams;
use tower_lsp::lsp_types::DidOpenTextDocumentParams;
use tower_lsp::lsp_types::TextDocumentItem;
use tower_lsp::lsp_types::WorkspaceFolder;
use tower_lsp::lsp_types::WorkspaceFoldersChangeEvent;
use url::Url;

use super::utils::test_client;
use super::utils::write_description;
use super::utils::write_sources;
use crate::lsp::backend::LspMessage;
use crate::lsp::backend::LspNotification;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::sources::SourceManager;
use crate::lsp::sources::SourceProvider;
use crate::lsp::sources::SourceRequest;
use crate::lsp::sources::SourceResponse;
use crate::lsp::state::WorldState;

/// A test [`SourceProvider`] that serves canned behavior per package name and
/// records every call, so tests can assert dispatch, dedup, and retry policy.
/// The provider is shared (as the `Arc<dyn SourceProvider>` the `SourceManager`
/// holds and the clone the test keeps), so `calls` is a plain `Mutex`.
struct TestProvider {
    /// Owns the cache directory that `Success` writes per-package sources into.
    sources: tempfile::TempDir,
    /// Per-package canned behavior.
    behavior: HashMap<String, TestBehavior>,
    /// Each request passed to `provide`, in call order.
    calls: Mutex<Vec<SourceRequest>>,
}

// Canned behavior to perform when a particular package is requested
enum TestBehavior {
    /// Write these `(basename, contents)` files into the package's source dir
    /// and return `Success(dir)`.
    Success(Vec<(&'static str, &'static str)>),
    Failed,
    Retry,
}

impl TestProvider {
    fn new(behavior: HashMap<String, TestBehavior>) -> Self {
        Self {
            sources: tempfile::tempdir().unwrap(),
            behavior,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// The requests passed to `provide`, in call order, for the test to assert on.
    fn calls(&self) -> &Mutex<Vec<SourceRequest>> {
        &self.calls
    }
}

impl SourceProvider for TestProvider {
    fn provide(&self, request: &SourceRequest) -> SourceResponse {
        self.calls.lock().unwrap().push(request.clone());

        match self.behavior.get(request.name()) {
            Some(TestBehavior::Success(files)) => {
                let dir = self.sources.path().join(request.name());
                write_sources(&dir, files);
                SourceResponse::Success(dir)
            },
            Some(TestBehavior::Retry) => SourceResponse::Retry,
            Some(TestBehavior::Failed) | None => SourceResponse::Failed,
        }
    }
}

fn did_change_workspace_folders(path: &Path) -> Event {
    Event::Lsp(LspMessage::Notification(
        LspNotification::DidChangeWorkspaceFolders(DidChangeWorkspaceFoldersParams {
            event: WorkspaceFoldersChangeEvent {
                added: vec![WorkspaceFolder {
                    uri: Url::from_file_path(path).unwrap(),
                    name: String::new(),
                }],
                removed: vec![],
            },
        }),
    ))
}

fn did_open(path: &Path, contents: &str) -> Event {
    Event::Lsp(LspMessage::Notification(
        LspNotification::DidOpenTextDocument(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: Url::from_file_path(path).unwrap(),
                language_id: String::from("r"),
                version: 0,
                text: contents.to_string(),
            },
        }),
    ))
}

/// The package names passed to the provider, in call order.
fn dispatched_names(calls: &Mutex<Vec<SourceRequest>>) -> Vec<String> {
    calls
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.name().to_string())
        .collect()
}

/// The happy path end to end: a workspace uses an installed library package via
/// `::`, so the revision-advance check dispatches a source request, the provider
/// returns a directory, and the main loop ingests it into the library package.
#[tokio::test]
async fn test_source_pipeline_ingests_package_sources() {
    let _aux = init_aux_for_test();

    let provider = Arc::new(TestProvider::new(HashMap::from([(
        String::from("donor"),
        TestBehavior::Success(vec![("foo.R", "foo <- function() 1\n")]),
    )])));

    // An installed library package with no `R/` sources of its own
    let lib = tempfile::tempdir().unwrap();
    write_description(&lib.path().join("donor"), "donor");
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);

    let mut state = GlobalState::from_parts(
        test_client(),
        WorldState::new(db, Library::new(vec![])),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            SourceManager::new(Some(provider.clone())),
        ),
    );

    // A workspace package that uses `donor` via `::`.
    let workspace = tempfile::tempdir().unwrap();
    let myproj = workspace.path().join("myproj");
    write_description(&myproj, "myproj");
    write_sources(&myproj.join("R"), &[("use.R", "donor::foo()\n")]);

    state
        .handle_event_to_quiescence(did_change_workspace_folders(workspace.path()))
        .await;

    // The provider was asked exactly once, with the package's name, version, and
    // library path extracted from the db on the main loop.
    {
        let recorded = provider.calls().lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].name(), "donor");
        assert_eq!(recorded[0].version(), "0.0.0");
        assert_eq!(recorded[0].library_path(), lib.path());
    }

    // `donor` now carries the ingested source file, readable from disk.
    let db = &state.world().db;
    let donor = db.package_by_name("donor").unwrap();
    let files = donor.files(db).clone();
    assert_eq!(files.len(), 1);
    assert!(files[0].source_text(db).contains("foo <- function()"));
}

/// A `Failed` fetch is terminal! Here, a later edit advances the revision, but the
/// package is not dispatched again.
#[tokio::test]
async fn test_failed_source_is_not_retried() {
    let _aux = init_aux_for_test();

    let provider = Arc::new(TestProvider::new(HashMap::from([(
        String::from("donor"),
        TestBehavior::Failed,
    )])));

    let lib = tempfile::tempdir().unwrap();
    write_description(&lib.path().join("donor"), "donor");
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);

    let mut state = GlobalState::from_parts(
        test_client(),
        WorldState::new(db, Library::new(vec![])),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            SourceManager::new(Some(provider.clone())),
        ),
    );

    let workspace = tempfile::tempdir().unwrap();
    let myproj = workspace.path().join("myproj");
    write_description(&myproj, "myproj");
    write_sources(&myproj.join("R"), &[("use.R", "donor::foo()\n")]);

    state
        .handle_event_to_quiescence(did_change_workspace_folders(workspace.path()))
        .await;

    // Ensure that we got the request once
    assert_eq!(dispatched_names(provider.calls()), vec![String::from(
        "donor"
    )]);

    // A later edit advances the revision, but the package is not retried.
    state
        .handle_event_to_quiescence(did_open(&workspace.path().join("other.R"), "1 + 1\n"))
        .await;

    // Ensure that we haven't gotten a second request
    assert_eq!(dispatched_names(provider.calls()), vec![String::from(
        "donor"
    )]);
}

/// A `Retry` fetch is transient. It writes nothing, and the package is
/// re-dispatched on the next revision-advancing edit.
#[tokio::test]
async fn test_retry_source_redispatches_on_next_edit() {
    let _aux = init_aux_for_test();

    let provider = Arc::new(TestProvider::new(HashMap::from([(
        String::from("donor"),
        TestBehavior::Retry,
    )])));

    let lib = tempfile::tempdir().unwrap();
    write_description(&lib.path().join("donor"), "donor");
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);

    let mut state = GlobalState::from_parts(
        test_client(),
        WorldState::new(db, Library::new(vec![])),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            SourceManager::new(Some(provider.clone())),
        ),
    );

    let workspace = tempfile::tempdir().unwrap();
    let myproj = workspace.path().join("myproj");
    write_description(&myproj, "myproj");
    write_sources(&myproj.join("R"), &[("use.R", "donor::foo()\n")]);

    state
        .handle_event_to_quiescence(did_change_workspace_folders(workspace.path()))
        .await;

    // Got the first one
    assert_eq!(dispatched_names(provider.calls()), vec![String::from(
        "donor"
    )]);

    // The next edit re-dispatches the transient `Retry`.
    state
        .handle_event_to_quiescence(did_open(&workspace.path().join("other.R"), "1 + 1\n"))
        .await;

    // Now we have two due to the retry
    assert_eq!(dispatched_names(provider.calls()), vec![
        String::from("donor"),
        String::from("donor")
    ]);

    // `Retry` never writes sources.
    let db = &state.world().db;
    let donor = db.package_by_name("donor").unwrap();
    assert!(donor.files(db).is_empty());
}
