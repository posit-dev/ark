//! Tests that drive the source request pipeline through the real [`GlobalState`]
//! event loop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use oak_db::Db;
use oak_db::OakDatabase;
use oak_scan::DbScan;

use super::source_handler::TestBehavior;
use super::source_handler::TestSourceHandler;
use super::utils::did_change_workspace_folders;
use super::utils::did_open;
use super::utils::test_client;
use super::utils::write_sources;
use super::utils::DescriptionWriter;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::sources::OakSourceHandler;
use crate::lsp::sources::SourceHandler;
use crate::lsp::sources::SourceRequest;
use crate::lsp::sources::SourceScheduler;
use crate::lsp::state::WorldState;

/// The package names passed to the handler, in call order.
fn dispatched_names(calls: &Mutex<Vec<SourceRequest>>) -> Vec<String> {
    calls
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.name().to_string())
        .collect()
}

/// Find R on the `PATH`
///
/// On Windows, `which` (from Git) returns POSIX paths that `Command::new()` can't resolve.
/// Use `where` which returns native paths.
fn find_r() -> PathBuf {
    let output = std::process::Command::new(if cfg!(windows) { "where" } else { "which" })
        .arg("R")
        .output()
        .unwrap_or_else(|err| panic!("Failed to find R: {err}"));
    assert!(output.status.success());

    // `where` on Windows can return multiple matches, take the first
    PathBuf::from(
        String::from_utf8(output.stdout)
            .expect("Non-UTF8 R path")
            .trim()
            .lines()
            .next()
            .expect("R should exist"),
    )
}

/// The happy path end to end: a workspace uses an installed library package via
/// `::`, so the revision-advance check dispatches a source request, the handler
/// returns a directory, and the main loop ingests it into the library package.
#[tokio::test]
async fn test_source_pipeline_ingests_package_sources() {
    let _aux = init_aux_for_test();

    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("donor"),
        TestBehavior::Success(vec![("foo.R", "foo <- function() 1\n")]),
    )])));

    // An installed library package with no `R/` sources of its own
    let lib = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("donor")
        .version("0.0.0")
        .built("dummy")
        .write(&lib.path().join("donor"));
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);

    let mut state = GlobalState::from_parts(
        test_client(),
        WorldState::new(db),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            SourceScheduler::new(Some(handler.clone())),
        ),
    );

    // A workspace package that uses `donor` via `::`.
    let workspace = tempfile::tempdir().unwrap();
    let myproj = workspace.path().join("myproj");
    DescriptionWriter::new()
        .package("myproj")
        .version("0.0.0")
        .write(&myproj);
    write_sources(&myproj.join("R"), &[("use.R", "donor::foo()\n")]);

    state
        .handle_event_to_quiescence(did_change_workspace_folders(workspace.path()))
        .await;

    // The handler was asked exactly once, with the package's name, version, and
    // library path extracted from the db on the main loop.
    {
        let recorded = handler.calls().lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].name(), "donor");
        assert_eq!(recorded[0].version(), "0.0.0");
        assert_eq!(recorded[0].built(), "dummy");
        assert_eq!(recorded[0].library_path(), lib.path());
    }

    // `donor` now carries the ingested source file, readable from disk.
    let db = &state.world().db;
    let donor = db.package_by_name("donor").unwrap();
    let files = donor.files(db).clone();
    assert_eq!(files.len(), 1);
    assert!(files[0].source_text(db).contains("foo <- function()"));
}

/// A `Failure` fetch is terminal! Here, a later edit advances the revision, but the
/// package is not dispatched again.
#[tokio::test]
async fn test_failed_source_is_not_retried() {
    let _aux = init_aux_for_test();

    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("donor"),
        TestBehavior::Failure,
    )])));

    let lib = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("donor")
        .version("0.0.0")
        .built("dummy")
        .write(&lib.path().join("donor"));
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);

    let mut state = GlobalState::from_parts(
        test_client(),
        WorldState::new(db),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            SourceScheduler::new(Some(handler.clone())),
        ),
    );

    let workspace = tempfile::tempdir().unwrap();
    let myproj = workspace.path().join("myproj");
    DescriptionWriter::new()
        .package("myproj")
        .version("0.0.0")
        .write(&myproj);
    write_sources(&myproj.join("R"), &[("use.R", "donor::foo()\n")]);

    state
        .handle_event_to_quiescence(did_change_workspace_folders(workspace.path()))
        .await;

    // Ensure that we got the request once
    assert_eq!(dispatched_names(handler.calls()), vec![String::from(
        "donor"
    )]);

    // A later edit advances the revision, but the package is not retried.
    state
        .handle_event_to_quiescence(did_open(&workspace.path().join("other.R"), "1 + 1\n"))
        .await;

    // Ensure that we haven't gotten a second request
    assert_eq!(dispatched_names(handler.calls()), vec![String::from(
        "donor"
    )]);
}

/// End to end against real `srcref` recovery: install {generics} from source into a
/// temporary library, point a workspace at it via `::`, inject the real
/// [`OakSourceHandler`], and assert the recovered sources are ingested.
///
/// Requires R on the `PATH` and internet access. We use {generics} because it is small and
/// easy to install from source, the same package `oak_srcref`'s own extraction test uses.
#[tokio::test]
async fn test_source_pipeline_ingests_real_srcref_sources() {
    let _aux = init_aux_for_test();

    let r = find_r();

    // Temporary library, with {generics} installed from source so srcrefs are preserved
    let library = tempfile::tempdir().unwrap();

    // Use forward slashes so the path is safe inside R string literals on Windows
    let library_literal = library.path().display().to_string().replace('\\', "/");

    let output = oak_r_process::run_text(
        &r,
        &format!(
            r#"install.packages("generics", lib = "{library_literal}", repos = "https://cran.r-project.org", type = "source", INSTALL_opts = "--with-keep.source")"#,
        ),
        &[],
        &[],
    )
    .expect("Failed to run install.packages()");
    assert!(output.status.success());

    // The real handler, with both caches rooted in a temp dir so the test doesn't touch
    // the shared on disk cache
    let cache = tempfile::tempdir().unwrap();
    let handler: Arc<dyn SourceHandler> =
        Arc::new(OakSourceHandler::new_in(cache.path(), r).unwrap());

    let mut db = OakDatabase::new();
    db.set_library_paths(&[library.path().to_path_buf()]);

    let mut state = GlobalState::from_parts(
        test_client(),
        WorldState::new(db),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            SourceScheduler::new(Some(handler)),
        ),
    );

    // A workspace package that uses {generics} via `::`
    let workspace = tempfile::tempdir().unwrap();
    let myproj = workspace.path().join("myproj");
    DescriptionWriter::new()
        .package("myproj")
        .version("0.0.0")
        .write(&myproj);
    write_sources(&myproj.join("R"), &[("use.R", "generics::as.factor()\n")]);

    state
        .handle_event_to_quiescence(did_change_workspace_folders(workspace.path()))
        .await;

    // {generics} now carries its recovered sources, readable from disk. {generics} is a
    // package of S3 generics, so every recovered file is full of `UseMethod()` calls.
    let db = &state.world().db;
    let generics = db.package_by_name("generics").unwrap();
    let files = generics.files(db).clone();
    assert!(!files.is_empty());
    assert!(files
        .iter()
        .any(|file| file.source_text(db).contains("UseMethod")));
}
