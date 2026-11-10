//! `pkg::` completions through the main loop, which mark their answer incomplete
//! while the package's sources or sidecar facts may still arrive.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use oak_db::OakDatabase;
use oak_db::SourceDb;
use oak_scan::DbScan;
use serde_json::json;
use tower_lsp_server::ls_types::CompletionItemKind;
use tower_lsp_server::ls_types::CompletionList;
use tower_lsp_server::ls_types::CompletionResponse;
use tower_lsp_server::ls_types::Position;

use super::sidecar_handler::TestSidecarHandler;
use super::source_handler::gate;
use super::source_handler::TestBehavior;
use super::source_handler::TestSourceHandler;
use super::utils::completion;
use super::utils::did_open;
use super::utils::initialize;
use super::utils::initialized;
use super::utils::source_scheduler_for_test;
use super::utils::test_client;
use super::utils::world_with_source_fetching;
use super::utils::DescriptionWriter;
use super::utils::TestClient;
use crate::lsp::backend::LspResponse;
use crate::lsp::backend::RequestResponse;
use crate::lsp::config::OAK_SOURCE_FETCHING_ENABLED_SETTING;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::sidecar::SidecarScheduler;
use crate::lsp::sources::SourceScheduler;

const FOO_SOURCES: &[(&str, &str)] = &[("foo.R", "foo <- function() 1\n")];

/// A library holding `donor`, an installed package without sources whose
/// `NAMESPACE` is `namespace`.
fn library_with_donor(namespace: &str) -> (tempfile::TempDir, OakDatabase) {
    let lib = tempfile::tempdir().unwrap();
    let donor = lib.path().join("donor");
    DescriptionWriter::new()
        .package("donor")
        .version("0.0.0")
        .built("dummy")
        .write(&donor);
    std::fs::write(donor.join("NAMESPACE"), namespace).unwrap();

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    (lib, db)
}

/// A library holding `donor` with `export(foo)` and an installed `base`, both
/// without sources.
fn library_with_base_and_donor() -> (tempfile::TempDir, OakDatabase) {
    let lib = tempfile::tempdir().unwrap();
    let donor = lib.path().join("donor");
    DescriptionWriter::new()
        .package("donor")
        .version("0.0.0")
        .built("dummy")
        .write(&donor);
    std::fs::write(donor.join("NAMESPACE"), "export(foo)\n").unwrap();
    DescriptionWriter::new()
        .package("base")
        .version("4.5.2")
        .built("dummy")
        .field("Priority", "base")
        .write(&lib.path().join("base"));

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    (lib, db)
}

fn state_with_scheduler(db: OakDatabase, scheduler: SourceScheduler) -> GlobalState {
    GlobalState::from_parts(
        test_client(),
        world_with_source_fetching(db),
        LspState::new(tokio::sync::mpsc::unbounded_channel().0).with_source_scheduler(scheduler),
    )
}

fn wait_entered(entered_rx: &Receiver<()>) {
    entered_rx.recv_timeout(Duration::from_secs(30)).unwrap();
}

async fn complete(state: &mut GlobalState, path: &Path, position: Position) -> CompletionResponse {
    let (event, mut response_rx) = completion(path, position);
    state.handle_event_once(event).await;

    match response_rx.try_recv() {
        Ok(RequestResponse::Result(Ok(LspResponse::Completion(Some(response))))) => response,
        Ok(RequestResponse::Result(response)) => {
            panic!("Unexpected completion response: {response:?}")
        },
        Ok(RequestResponse::Disabled) => panic!("The completion request is disabled"),
        Err(err) => panic!("No completion response: {err:?}"),
    }
}

fn complete_items(response: CompletionResponse) -> Vec<(String, Option<CompletionItemKind>)> {
    let CompletionResponse::Array(items) = response else {
        panic!("Expected a complete answer, got: {response:?}");
    };
    items
        .into_iter()
        .map(|item| (item.label, item.kind))
        .collect()
}

fn incomplete_items(response: CompletionResponse) -> Vec<(String, Option<CompletionItemKind>)> {
    let CompletionResponse::List(CompletionList {
        is_incomplete: true,
        items,
    }) = response
    else {
        panic!("Expected an incomplete answer, got: {response:?}");
    };
    items
        .into_iter()
        .map(|item| (item.label, item.kind))
        .collect()
}

fn foo(kind: CompletionItemKind) -> Vec<(String, Option<CompletionItemKind>)> {
    vec![(String::from("foo"), Some(kind))]
}

/// While the fetch is blocked, exports have unknown kinds and the answer is
/// incomplete. After ingestion, `foo` has function kind and the answer is complete.
#[tokio::test]
async fn test_namespace_answer_is_incomplete_until_sources_land() {
    let _aux = init_aux_for_test();

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (gate, release) = gate(entered_tx);
    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("donor"),
        TestBehavior::GatedSuccess(gate, FOO_SOURCES.to_vec()),
    )])));

    let (_lib, db) = library_with_donor("export(foo)\n");
    let mut state = state_with_scheduler(db, source_scheduler_for_test(handler));

    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("scratch.R");
    state
        .handle_event_once(did_open(&path, "donor::foo()\ndonor::\n"))
        .await;
    wait_entered(&entered_rx);

    let response = complete(&mut state, &path, Position::new(1, 7)).await;
    assert_eq!(incomplete_items(response), foo(CompletionItemKind::STRUCT));

    drop(release);
    state.pump_sources_to_quiescence().await;

    let response = complete(&mut state, &path, Position::new(1, 7)).await;
    assert_eq!(complete_items(response), foo(CompletionItemKind::FUNCTION));
}

/// A bare `donor::` names `donor` as a dependency, so opening a file with
/// nothing else in it schedules the fetch. Once the fetch fails, no sources can
/// arrive and the answer is complete.
#[tokio::test]
async fn test_bare_namespace_access_schedules_fetch() {
    let _aux = init_aux_for_test();

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (gate, release) = gate(entered_tx);
    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("donor"),
        TestBehavior::Gated(gate),
    )])));

    let (_lib, db) = library_with_donor("export(foo)\n");
    let mut state = state_with_scheduler(db, source_scheduler_for_test(handler.clone()));

    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("scratch.R");
    state.handle_event_once(did_open(&path, "donor::\n")).await;
    wait_entered(&entered_rx);

    let names: Vec<String> = handler
        .calls()
        .lock()
        .unwrap()
        .iter()
        .map(|request| request.name().to_string())
        .collect();
    assert_eq!(names, vec![String::from("donor")]);

    let response = complete(&mut state, &path, Position::new(0, 7)).await;
    assert_eq!(incomplete_items(response), foo(CompletionItemKind::STRUCT));

    drop(release);
    state.pump_sources_to_quiescence().await;

    let response = complete(&mut state, &path, Position::new(0, 7)).await;
    assert_eq!(complete_items(response), foo(CompletionItemKind::STRUCT));
}

/// An empty `donor:::` answer while the fetch is held is an incomplete list,
/// since `null` would stop the editor from asking again once names arrive.
#[tokio::test]
async fn test_empty_pending_answer_is_an_incomplete_list() {
    let _aux = init_aux_for_test();

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (gate, release) = gate(entered_tx);
    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("donor"),
        TestBehavior::Gated(gate),
    )])));

    let (_lib, db) = library_with_donor("");
    let mut state = state_with_scheduler(db, source_scheduler_for_test(handler));

    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("scratch.R");
    state.handle_event_once(did_open(&path, "donor:::\n")).await;
    wait_entered(&entered_rx);

    let response = complete(&mut state, &path, Position::new(0, 8)).await;
    assert_eq!(
        response,
        CompletionResponse::List(CompletionList {
            is_incomplete: true,
            items: vec![],
        })
    );

    drop(release);
    state.pump_sources_to_quiescence().await;
}

/// With fetching turned off, no sources can arrive and the answer is complete.
#[tokio::test]
async fn test_namespace_answer_is_complete_with_fetching_off() {
    let _aux = init_aux_for_test();

    let handler = Arc::new(TestSourceHandler::new(HashMap::from([(
        String::from("donor"),
        TestBehavior::Success(FOO_SOURCES.to_vec()),
    )])));

    let (_lib, db) = library_with_donor("export(foo)\n");
    let client = TestClient::new(&[(OAK_SOURCE_FETCHING_ENABLED_SETTING, json!(false))]).await;
    let mut state = GlobalState::from_parts(
        client.client(),
        world_with_source_fetching(db),
        LspState::new(tokio::sync::mpsc::unbounded_channel().0)
            .with_source_scheduler(SourceScheduler::new(Some(handler.clone()))),
    );

    let workspace = tempfile::tempdir().unwrap();
    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;
    state.handle_event_to_quiescence(initialized()).await;

    let path = workspace.path().join("scratch.R");
    state
        .handle_event_to_quiescence(did_open(&path, "donor::\n"))
        .await;
    assert!(handler.calls().lock().unwrap().is_empty());

    let response = complete(&mut state, &path, Position::new(0, 7)).await;
    assert_eq!(complete_items(response), foo(CompletionItemKind::STRUCT));
}

/// Without a source handler, no sources can arrive and the answer is complete.
#[tokio::test]
async fn test_namespace_answer_is_complete_without_handler() {
    let _aux = init_aux_for_test();

    let (_lib, db) = library_with_donor("export(foo)\n");
    let mut scheduler = SourceScheduler::new(None);
    scheduler.config_arrived();
    let mut state = state_with_scheduler(db, scheduler);

    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("scratch.R");
    state
        .handle_event_to_quiescence(did_open(&path, "donor::\n"))
        .await;

    let response = complete(&mut state, &path, Position::new(0, 7)).await;
    assert_eq!(complete_items(response), foo(CompletionItemKind::STRUCT));
}

/// Disables source fetching and holds the sidecar response until the returned
/// sender is dropped, isolating its effect on completion-list completeness.
fn state_with_held_sidecar(
    db: OakDatabase,
    lazy_data: &[(&str, &[&str])],
) -> (GlobalState, std::sync::mpsc::Sender<()>, Receiver<()>) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (gate, release) = gate(entered_tx);
    let mut sidecar = TestSidecarHandler::gated(gate, vec![(
        "primitives.R",
        "sum <- function(..., na.rm = FALSE) NULL\n",
    )]);
    for (name, names) in lazy_data {
        let package = db.package_by_name(name).unwrap();
        sidecar = sidecar.with_lazy_data(package, names);
    }
    let sidecar = Arc::new(sidecar);

    let mut scheduler = SourceScheduler::new(None);
    scheduler.config_arrived();
    let state = GlobalState::from_parts(
        test_client(),
        world_with_source_fetching(db),
        LspState::new(tokio::sync::mpsc::unbounded_channel().0)
            .with_source_scheduler(scheduler)
            .with_sidecar_scheduler(SidecarScheduler::new(Some(sidecar))),
    );
    (state, release, entered_rx)
}

/// `base::` is incomplete while the sidecar job, which can add its primitive
/// stubs, is running, and complete once the stubs land.
#[tokio::test]
async fn test_base_answer_is_incomplete_while_sidecar_runs() {
    let _aux = init_aux_for_test();

    let (_lib, db) = library_with_base_and_donor();
    let (mut state, release, entered_rx) = state_with_held_sidecar(db, &[]);

    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("scratch.R");
    state.handle_event_once(did_open(&path, "base::\n")).await;
    wait_entered(&entered_rx);

    let response = complete(&mut state, &path, Position::new(0, 6)).await;
    assert_eq!(incomplete_items(response), vec![]);

    drop(release);
    state.pump_sidecar_to_quiescence().await;

    let response = complete(&mut state, &path, Position::new(0, 6)).await;
    assert_eq!(complete_items(response), vec![(
        String::from("sum"),
        Some(CompletionItemKind::FUNCTION)
    )]);
}

/// The running sidecar can change answers only for packages in its request.
/// Other packages remain complete when source fetching is disabled.
#[tokio::test]
async fn test_sidecar_job_leaves_other_packages_complete() {
    let _aux = init_aux_for_test();

    let (_lib, db) = library_with_base_donor_and_datasets();
    let (mut state, release, entered_rx) = state_with_held_sidecar(db, &[]);

    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("scratch.R");
    state.handle_event_once(did_open(&path, "donor::\n")).await;
    wait_entered(&entered_rx);

    let response = complete(&mut state, &path, Position::new(0, 7)).await;
    assert_eq!(complete_items(response), foo(CompletionItemKind::STRUCT));

    drop(release);
    state.pump_sidecar_to_quiescence().await;
}

#[tokio::test]
async fn test_lazy_data_answer_is_incomplete_while_sidecar_runs() {
    let _aux = init_aux_for_test();

    let (_lib, db) = library_with_base_donor_and_datasets();
    let (mut state, release, entered_rx) =
        state_with_held_sidecar(db, &[("datasets", &["mtcars", "BJsales.lead"])]);

    let scratch = tempfile::tempdir().unwrap();
    let path = scratch.path().join("scratch.R");
    state
        .handle_event_once(did_open(&path, "datasets::\ndatasets:::\n"))
        .await;
    wait_entered(&entered_rx);

    let response = complete(&mut state, &path, Position::new(0, 10)).await;
    assert_eq!(incomplete_items(response), vec![]);

    drop(release);
    state.pump_sidecar_to_quiescence().await;

    let response = complete(&mut state, &path, Position::new(0, 10)).await;
    assert_eq!(complete_items(response), vec![
        (
            String::from("BJsales.lead"),
            Some(CompletionItemKind::STRUCT)
        ),
        (String::from("mtcars"), Some(CompletionItemKind::STRUCT)),
    ]);

    // The protocol represents a complete empty answer as `null`.
    let (event, mut response_rx) = completion(&path, Position::new(1, 11));
    state.handle_event_once(event).await;
    assert!(matches!(
        response_rx.try_recv(),
        Ok(RequestResponse::Result(Ok(LspResponse::Completion(None))))
    ));
}

/// The empty `Rdata.rds` makes `datasets` eligible for the sidecar request.
/// [`TestSidecarHandler`] supplies the names without reading the file.
fn library_with_base_donor_and_datasets() -> (tempfile::TempDir, OakDatabase) {
    let (lib, _db) = library_with_base_and_donor();

    let datasets = lib.path().join("datasets");
    DescriptionWriter::new()
        .package("datasets")
        .version("4.5.2")
        .built("dummy")
        .write(&datasets);
    std::fs::create_dir_all(datasets.join("data")).unwrap();
    std::fs::write(datasets.join("data").join("Rdata.rds"), "").unwrap();

    // Setting the same library path again does not rescan newly added packages.
    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    (lib, db)
}
