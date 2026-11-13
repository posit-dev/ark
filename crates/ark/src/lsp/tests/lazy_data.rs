//! Tests for selecting installed packages with lazy data indexes and
//! supplying their object names through the sidecar job.

use std::sync::Arc;

use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::SourceDb;
use oak_scan::DbScan;
use oak_sidecar::primitives::PrimitivesCache;
use tower_lsp_server::ls_types::FileChangeType;

use super::sidecar_handler::install_package;
use super::sidecar_handler::sidecar_state;
use super::sidecar_handler::TestSidecarHandler;
use super::utils::did_change_watched_file;
use super::utils::initialize;
use super::utils::DescriptionWriter;
use crate::lsp::harness::TestEvent;
use crate::lsp::harness::TestHandlers;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::sidecar::OakSidecarHandler;
use crate::lsp::sidecar::SidecarHandler;
use crate::lsp::sidecar::SidecarRequest;
use crate::panic;

#[test]
fn test_sidecar_reads_lazy_data_like_read_rds() {
    let r_home = harp::command::r_home_setup().unwrap();
    let r = oak_r_process::r_executable(&r_home).unwrap();

    let output = oak_r_process::run_text(
        &r,
        r#"
path <- system.file("data", "Rdata.rds", package = "datasets", mustWork = TRUE)
names <- unlist(readRDS(path), use.names = FALSE)
writeLines(sort(unique(names), method = "radix"))
"#,
        &oak_r_process::RunOptions::default(),
    )
    .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let expected: Vec<&str> = stdout.lines().collect();

    // `BJsales` creates two objects, so the dataset name alone is not enough.
    assert!(expected.contains(&"BJsales"));
    assert!(expected.contains(&"BJsales.lead"));

    let mut db = OakDatabase::new();
    db.set_library_paths(&[r_home.join("library")]);
    let datasets = db.package_by_name("datasets").unwrap();
    let utils = db.package_by_name("utils").unwrap();

    let request = SidecarRequest::from_db(&db);
    let passed: Vec<Package> = request
        .packages()
        .iter()
        .map(|package| package.package())
        .collect();
    assert!(passed.contains(&datasets));
    assert!(!passed.contains(&utils));

    let cache = tempfile::tempdir().unwrap();
    let primitives = PrimitivesCache::open_in(cache.path().to_path_buf(), r.clone()).unwrap();
    let handler = OakSidecarHandler::with_primitives(r, primitives);
    let response = handler.handle(&request);

    let facts = response
        .packages
        .into_iter()
        .find(|facts| facts.package == datasets)
        .unwrap();
    datasets.set_lazy_data(&mut db, facts.lazy_data.unwrap());
    assert_eq!(datasets.lazy_data(&db), expected);
}

#[tokio::test]
async fn test_packages_without_rdata_are_not_passed() {
    let _aux = init_aux_for_test();

    let lib = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("base")
        .version("4.5.2")
        .built("dummy")
        .field("Priority", "base")
        .write_installed(&lib.path().join("base"));
    DescriptionWriter::new()
        .package("nodata")
        .version("0.0.0")
        .built("dummy")
        .write_installed(&lib.path().join("nodata"));
    let withdata = lib.path().join("withdata");
    DescriptionWriter::new()
        .package("withdata")
        .version("0.0.0")
        .built("dummy")
        .write_installed(&withdata);
    std::fs::create_dir_all(withdata.join("data")).unwrap();
    std::fs::write(withdata.join("data").join("Rdata.rds"), "").unwrap();

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let base = db.package_by_name("base").unwrap();
    let nodata = db.package_by_name("nodata").unwrap();
    let withdata = db.package_by_name("withdata").unwrap();

    let sidecar =
        Arc::new(TestSidecarHandler::new(Vec::new()).with_lazy_data("withdata", &["obj"]));
    let mut state = sidecar_state(db, Arc::clone(&sidecar));

    let workspace = tempfile::tempdir().unwrap();
    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;

    // A guard can't be held across `.await`, so each synchronous db section
    // gets its own.
    let _boundary = panic::test_boundary();
    let calls = sidecar.calls().lock().unwrap();
    assert_eq!(calls.len(), 1);
    let mut passed: Vec<(Package, bool, bool)> = calls[0]
        .packages()
        .iter()
        .map(|package| {
            (
                package.package(),
                package.has_primitives(),
                package.has_lazy_data(),
            )
        })
        .collect();
    passed.sort_by_key(|(package, _, _)| package.name(state.world().db()).clone());
    assert_eq!(passed, vec![(base, true, false), (withdata, false, true)]);

    let db = state.world().db();
    assert!(base.lazy_data(db).is_empty());
    assert!(nodata.lazy_data(db).is_empty());
    assert_eq!(withdata.lazy_data(db), ["obj"]);
}

/// A session with `base` and `withdata` after the initialization batch has
/// completed. The test handler supplies `names` for `withdata`.
async fn session_with_withdata(
    names: &[&str],
) -> (
    tempfile::TempDir,
    tempfile::TempDir,
    GlobalState<TestHandlers>,
    Arc<TestSidecarHandler>,
) {
    let lib = tempfile::tempdir().unwrap();
    install_package(lib.path(), "base", 1_000, false);
    install_package(lib.path(), "withdata", 1_000, true);
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);

    let sidecar = Arc::new(TestSidecarHandler::new(Vec::new()).with_lazy_data("withdata", names));
    let mut state = sidecar_state(db, Arc::clone(&sidecar));

    let workspace = tempfile::tempdir().unwrap();
    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;

    (lib, workspace, state, sidecar)
}

#[tokio::test]
async fn test_install_after_startup_gets_lazy_data_names() {
    let _aux = init_aux_for_test();
    let (lib, _workspace, mut state, sidecar) = session_with_withdata(&[]).await;
    sidecar.set_lazy_data("late", &["obj"]);

    install_package(lib.path(), "late", 1_000, true);
    state
        .handle_event_to_quiescence(did_change_watched_file(
            &lib.path().join("late"),
            FileChangeType::CREATED,
        ))
        .await;

    let _boundary = panic::test_boundary();
    let db = state.world().db();
    let late = db.package_by_name("late").unwrap();
    assert_eq!(late.lazy_data(db), ["obj"]);
    assert_eq!(late.facts_revision(db), Some(late.description_revision(db)));
    assert_eq!(sidecar.calls().lock().unwrap().len(), 2);
}

/// The upgraded package keeps its `Package` entity and its old names until
/// the next batch replaces them.
#[tokio::test]
async fn test_in_place_upgrade_replaces_lazy_data_names() {
    let _aux = init_aux_for_test();
    let (lib, _workspace, mut state, sidecar) = session_with_withdata(&["old"]).await;
    let withdata = {
        let _boundary = panic::test_boundary();
        let withdata = state.world().db().package_by_name("withdata").unwrap();
        assert_eq!(withdata.lazy_data(state.world().db()), ["old"]);
        withdata
    };

    sidecar.set_lazy_data("withdata", &["new"]);
    install_package(lib.path(), "withdata", 2_000, true);
    state
        .handle_event_to_quiescence(did_change_watched_file(
            &lib.path().join("withdata"),
            FileChangeType::CREATED,
        ))
        .await;

    let _boundary = panic::test_boundary();
    let db = state.world().db();
    assert_eq!(db.package_by_name("withdata"), Some(withdata));
    assert_eq!(withdata.lazy_data(db), ["new"]);
    assert_eq!(
        withdata.facts_revision(db),
        Some(withdata.description_revision(db))
    );
    assert_eq!(sidecar.calls().lock().unwrap().len(), 2);
}

/// A new version without `Rdata.rds` has no lazy data. It is covered with an
/// empty list without a batch, which removes the old version's names.
#[tokio::test]
async fn test_upgrade_without_rdata_clears_lazy_data_names() {
    let _aux = init_aux_for_test();
    let (lib, _workspace, mut state, sidecar) = session_with_withdata(&["old"]).await;
    let withdata = {
        let _boundary = panic::test_boundary();
        let withdata = state.world().db().package_by_name("withdata").unwrap();
        assert_eq!(withdata.lazy_data(state.world().db()), ["old"]);
        withdata
    };

    install_package(lib.path(), "withdata", 2_000, false);
    state
        .handle_event_to_quiescence(did_change_watched_file(
            &lib.path().join("withdata"),
            FileChangeType::CREATED,
        ))
        .await;

    let _boundary = panic::test_boundary();
    let db = state.world().db();
    assert!(withdata.lazy_data(db).is_empty());
    assert_eq!(
        withdata.facts_revision(db),
        Some(withdata.description_revision(db))
    );
    assert_eq!(sidecar.calls().lock().unwrap().len(), 1);
}

/// Console inputs can arrive before the LSP `initialize` request. An empty
/// library submits no batch and does not prevent requests for later installs.
#[tokio::test]
async fn test_event_before_initialize_submits_nothing() {
    let _aux = init_aux_for_test();

    let lib = tempfile::tempdir().unwrap();
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    let sidecar =
        Arc::new(TestSidecarHandler::new(Vec::new()).with_lazy_data("withdata", &["obj"]));
    let mut state = sidecar_state(db, Arc::clone(&sidecar));

    let console_inputs = Event::Host(TestEvent::SessionScopes(vec![]));
    state.handle_event_once(console_inputs).await;
    assert!(sidecar.calls().lock().unwrap().is_empty());
    assert!(!state.lsp_state().sidecar_scheduler.has_pending());

    let workspace = tempfile::tempdir().unwrap();
    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;
    assert!(sidecar.calls().lock().unwrap().is_empty());

    install_package(lib.path(), "withdata", 1_000, true);
    state
        .handle_event_to_quiescence(did_change_watched_file(
            &lib.path().join("withdata"),
            FileChangeType::CREATED,
        ))
        .await;

    let _boundary = panic::test_boundary();
    let db = state.world().db();
    let withdata = db.package_by_name("withdata").unwrap();
    assert_eq!(withdata.lazy_data(db), ["obj"]);
    assert_eq!(sidecar.calls().lock().unwrap().len(), 1);
}
