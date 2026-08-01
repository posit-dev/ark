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
use super::utils::initialize;
use super::utils::initialized;
use super::utils::source_scheduler_for_test;
use super::utils::test_client;
use super::utils::world_with_source_fetching;
use super::utils::write_sources;
use super::utils::DescriptionWriter;
use crate::lsp::config::apply_env_overrides;
use crate::lsp::config::LspConfig;
use crate::lsp::config::OAK_SOURCE_FETCHING_ENABLED_ENV_VAR;
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
        world_with_source_fetching(db),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            source_scheduler_for_test(handler.clone()),
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

/// Disabling `oak.sourceFetching.enabled` leaves dependency discovery intact
/// but dispatches no source request.
#[tokio::test]
async fn test_disabled_source_fetching_dispatches_nothing() {
    let _aux = init_aux_for_test();

    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("donor"),
        TestBehavior::Success(vec![("foo.R", "foo <- function() 1\n")]),
    )])));

    let lib = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("donor")
        .version("0.0.0")
        .built("dummy")
        .write(&lib.path().join("donor"));
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);

    let mut world = WorldState::new(db);
    world.config.oak.source_fetching_enabled = false;

    let mut state = GlobalState::from_parts(
        test_client(),
        world,
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            source_scheduler_for_test(handler.clone()),
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

    assert!(handler.calls().lock().unwrap().is_empty());

    // The dependency is indexed even though its sources were not fetched.
    let db = &state.world().db;
    let donor = db.package_by_name("donor").unwrap();
    assert!(donor.files(db).is_empty());
}

/// Turning the setting back on fetches the packages Oak saw while it was off,
/// which is what `doc/configuration-oak.md` promises. This works because both
/// early returns in `schedule()` come before the loop that records a package,
/// so a declined package stays unseen rather than being marked `Finished`.
///
/// In production `update_config()` bumps the revision itself, so the fetch
/// starts on the config change alone. The `did_open()` here stands in for that
/// bump, which a test can't reach without a client that answers
/// `workspace/configuration`.
#[tokio::test]
async fn test_reenabling_fetches_packages_seen_while_off() {
    let _aux = init_aux_for_test();

    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("donor"),
        TestBehavior::Success(vec![("foo.R", "foo <- function() 1\n")]),
    )])));

    let lib = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("donor")
        .version("0.0.0")
        .built("dummy")
        .write(&lib.path().join("donor"));
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);

    let mut world = WorldState::new(db);
    world.config.oak.source_fetching_enabled = false;

    let mut state = GlobalState::from_parts(
        test_client(),
        world,
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            source_scheduler_for_test(handler.clone()),
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
    assert!(handler.calls().lock().unwrap().is_empty());

    state.world_mut().config.oak.source_fetching_enabled = true;
    state
        .handle_event_to_quiescence(did_open(&workspace.path().join("other.R"), "1 + 1\n"))
        .await;

    // `donor` was declined while off, so it is still on offer and gets fetched
    // now, sources and all.
    assert_eq!(dispatched_names(handler.calls()), vec!["donor"]);
    let db = &state.world().db;
    let donor = db.package_by_name("donor").unwrap();
    let files = donor.files(db).clone();
    assert_eq!(files.len(), 1);
    assert!(files[0].source_text(db).contains("foo <- function()"));
}

/// Turning the setting off mid-session stops fetching for a dependency that
/// turns up afterwards, without disturbing the one already fetched.
#[tokio::test]
async fn test_disabling_stops_fetching_new_packages() {
    let _aux = init_aux_for_test();

    let handler = Arc::new(TestSourceHandler::new(HashMap::from([
        (
            String::from("donor1"),
            TestBehavior::Success(vec![("foo.R", "foo <- function() 1\n")]),
        ),
        (
            String::from("donor2"),
            TestBehavior::Success(vec![("bar.R", "bar <- function() 2\n")]),
        ),
    ])));

    let lib = tempfile::tempdir().unwrap();
    for name in ["donor1", "donor2"] {
        DescriptionWriter::new()
            .package(name)
            .version("0.0.0")
            .built("dummy")
            .write(&lib.path().join(name));
    }
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);

    let mut state = GlobalState::from_parts(
        test_client(),
        world_with_source_fetching(db),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            source_scheduler_for_test(handler.clone()),
        ),
    );

    // Two workspace folders, each depending on a different library package, so
    // the second dependency only appears once the second folder is added.
    let first = tempfile::tempdir().unwrap();
    let proj1 = first.path().join("proj1");
    DescriptionWriter::new()
        .package("proj1")
        .version("0.0.0")
        .write(&proj1);
    write_sources(&proj1.join("R"), &[("use.R", "donor1::foo()\n")]);

    let second = tempfile::tempdir().unwrap();
    let proj2 = second.path().join("proj2");
    DescriptionWriter::new()
        .package("proj2")
        .version("0.0.0")
        .write(&proj2);
    write_sources(&proj2.join("R"), &[("use.R", "donor2::bar()\n")]);

    state
        .handle_event_to_quiescence(did_change_workspace_folders(first.path()))
        .await;
    assert_eq!(dispatched_names(handler.calls()), vec!["donor1"]);

    state.world_mut().config.oak.source_fetching_enabled = false;
    state
        .handle_event_to_quiescence(did_change_workspace_folders(second.path()))
        .await;

    // `donor2` was discovered by the second scan but never dispatched, and
    // `donor1` keeps the sources it already has.
    assert_eq!(dispatched_names(handler.calls()), vec!["donor1"]);
    let db = &state.world().db;
    assert!(db.package_by_name("donor2").unwrap().files(db).is_empty());
    assert_eq!(db.package_by_name("donor1").unwrap().files(db).len(), 1);
}

/// Do not fetch packages found during `initialize()` before attempting client configuration.
/// Release the startup gate after a failed configuration request.
#[tokio::test]
async fn test_fetching_waits_for_initialized() {
    let _aux = init_aux_for_test();

    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("donor"),
        TestBehavior::Success(vec![("foo.R", "foo <- function() 1\n")]),
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
        world_with_source_fetching(db),
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

    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;

    // The initial scan discovered the dependency without fetching its sources.
    assert!(handler.calls().lock().unwrap().is_empty());
    let db = &state.world().db;
    let donor = db.package_by_name("donor").unwrap();
    assert!(donor.files(db).is_empty());

    state.handle_event_to_quiescence(initialized()).await;
    assert_eq!(dispatched_names(handler.calls()), vec!["donor"]);
}

/// A recognized `OAK_SOURCE_FETCHING_ENABLED` value overrides `oak.sourceFetching.enabled`.
/// Unset or unrecognized values preserve the LSP setting.
#[test]
fn test_env_var_overrides_the_setting() {
    let name = OAK_SOURCE_FETCHING_ENABLED_ENV_VAR;

    let resolve = |client_says: bool| {
        let mut config = LspConfig::default();
        config.oak.source_fetching_enabled = client_says;
        apply_env_overrides(&mut config);
        config.oak.source_fetching_enabled
    };

    unsafe { std::env::remove_var(name) };
    assert!(resolve(true));
    assert!(!resolve(false));

    unsafe { std::env::set_var(name, "1") };
    assert!(resolve(true));
    assert!(resolve(false));

    unsafe { std::env::set_var(name, "0") };
    assert!(!resolve(true));
    assert!(!resolve(false));

    unsafe { std::env::set_var(name, "yes") };
    assert!(resolve(true));
    assert!(!resolve(false));

    unsafe { std::env::remove_var(name) };
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
        world_with_source_fetching(db),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            source_scheduler_for_test(handler.clone()),
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
        world_with_source_fetching(db),
        LspState::new(
            tokio::sync::mpsc::unbounded_channel().0,
            source_scheduler_for_test(handler),
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
