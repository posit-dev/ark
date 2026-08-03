//! Tests that drive the source request pipeline through the real [`GlobalState`]
//! event loop.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

use oak_db::Db;
use oak_db::OakDatabase;
use oak_scan::DbScan;
use tokio::sync::mpsc::UnboundedReceiver;

use super::source_handler::gate;
use super::source_handler::TestBehavior;
use super::source_handler::TestSourceHandler;
use super::utils::did_change_workspace_folders;
use super::utils::did_open;
use super::utils::initialize;
use super::utils::initialize_without_configuration;
use super::utils::initialized;
use super::utils::source_scheduler_for_test;
use super::utils::test_client;
use super::utils::world_with_source_fetching;
use super::utils::write_sources;
use super::utils::DescriptionWriter;
use crate::lsp::backend::RequestResponse;
use crate::lsp::config::apply_env_overrides;
use crate::lsp::config::LspConfig;
use crate::lsp::config::OAK_SOURCE_FETCHING_ENABLED_ENV_VAR;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::main_loop::SOURCE_POOL_THREADS;
use crate::lsp::sources::source_fetching_disabled_by_ci;
use crate::lsp::sources::OakSourceHandler;
use crate::lsp::sources::SourceHandler;
use crate::lsp::sources::SourceRequest;
use crate::lsp::sources::SourceScheduler;

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
        world_with_source_fetching(db, true),
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

    let mut state = GlobalState::from_parts(
        test_client(),
        world_with_source_fetching(db, false),
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

    let mut state = GlobalState::from_parts(
        test_client(),
        world_with_source_fetching(db, false),
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
        world_with_source_fetching(db, true),
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

/// Turning the setting off also stops the fetches still sitting in the pool's
/// queue. A fetch already inside the handler can't be interrupted, so what we
/// assert is that the queued one never reaches it, and that it stays on offer
/// for when fetching comes back on.
#[tokio::test]
async fn test_disabling_skips_queued_fetches() {
    let _aux = init_aux_for_test();

    // One gated package per source worker, plus the one that has to wait in the
    // queue. They share an "entered" sender because which of them a worker picks
    // up is unpredictable.
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let donors: Vec<String> = (0..SOURCE_POOL_THREADS + 1)
        .map(|i| format!("donor{i}"))
        .collect();

    let mut behavior = HashMap::new();
    let mut releases = Vec::new();
    for name in &donors {
        let (gate, release) = gate(entered_tx.clone());
        behavior.insert(name.clone(), TestBehavior::Gated(gate));
        releases.push(release);
    }
    let handler = Arc::new(TestSourceHandler::new(behavior));

    let lib = tempfile::tempdir().unwrap();
    for name in &donors {
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
        world_with_source_fetching(db, true),
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
    let uses: String = donors
        .iter()
        .map(|name| format!("{name}::foo()\n"))
        .collect();
    write_sources(&myproj.join("R"), &[("use.R", &uses)]);

    state
        .handle_event_once(did_change_workspace_folders(workspace.path()))
        .await;
    state.pump_scans_to_quiescence().await;

    // Every worker is now parked in `handle()`. A parked worker can't pick up
    // another job, so the last fetch is stuck in the queue until we release
    // them, which is what makes the rest of this test deterministic.
    for _ in 0..SOURCE_POOL_THREADS {
        entered_rx.recv().unwrap();
    }

    // The new setting reaches the queued job through the `schedule()` call that
    // ends this tick.
    state.world_mut().config.oak.source_fetching_enabled = false;
    state
        .handle_event_once(did_open(&workspace.path().join("script.R"), "x <- 1\n"))
        .await;

    // Let the parked fetches through and collect every response.
    drop(releases);
    state.pump_sources_to_quiescence().await;
    assert_eq!(handler.calls().lock().unwrap().len(), SOURCE_POOL_THREADS);

    state.world_mut().config.oak.source_fetching_enabled = true;
    state
        .handle_event_to_quiescence(did_open(&workspace.path().join("other.R"), "x <- 2\n"))
        .await;

    // The skipped package was left on offer, so it gets fetched now. The ones
    // that ran while fetching was on aren't asked for twice.
    let mut names = dispatched_names(handler.calls());
    names.sort();
    assert_eq!(names, donors);
}

/// Do not fetch packages found during `initialize()` before attempting client configuration.
/// Release the startup gate after a failed configuration request.
#[tokio::test]
async fn test_fetching_waits_for_initialized() {
    check_fetching_waits_for_initialized(initialize).await;
}

/// A client that doesn't support `workspace/configuration` is never asked for
/// settings, so the gate releases on the defaults instead of waiting forever.
#[tokio::test]
async fn test_fetching_waits_for_initialized_without_configuration() {
    check_fetching_waits_for_initialized(initialize_without_configuration).await;
}

async fn check_fetching_waits_for_initialized(
    initialize: fn(&Path) -> (Event, UnboundedReceiver<RequestResponse>),
) {
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
        world_with_source_fetching(db, true),
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

/// CI holds fetching back unless the env var opts a job back in. Both
/// `source_handler()` and the output-channel report in `handle_initialized()`
/// read this, so they always agree on whether CI is the reason.
#[test]
fn test_ci_holds_source_fetching_back_unless_opted_in() {
    let name = OAK_SOURCE_FETCHING_ENABLED_ENV_VAR;

    unsafe { std::env::set_var("CI", "true") };

    unsafe { std::env::remove_var(name) };
    assert!(source_fetching_disabled_by_ci());

    unsafe { std::env::set_var(name, "1") };
    assert!(!source_fetching_disabled_by_ci());

    // An explicit `0` leaves CI's own suppression in place rather than fighting
    // it, since both point the same way.
    unsafe { std::env::set_var(name, "0") };
    assert!(source_fetching_disabled_by_ci());

    // Off CI the gate never applies, whatever the variable says.
    unsafe { std::env::remove_var("CI") };
    assert!(!source_fetching_disabled_by_ci());
    unsafe { std::env::remove_var(name) };
    assert!(!source_fetching_disabled_by_ci());
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
        world_with_source_fetching(db, true),
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
        world_with_source_fetching(db, true),
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
