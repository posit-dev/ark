//! Sidecar scheduling across package installs, upgrades, removals, and failed
//! requests.

use std::path::Path;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use oak_db::FileRevision;
use oak_db::OakDatabase;
use oak_db::Package;
use oak_db::SourceDb;
use oak_scan::DbScan;
use tower_lsp_server::ls_types::FileChangeType;

use super::sidecar_handler::install_package;
use super::sidecar_handler::sidecar_state;
use super::sidecar_handler::TestSidecarHandler;
use super::source_handler::gate;
use super::utils::did_change_watched_file;
use super::utils::did_open;
use super::utils::DescriptionWriter;
use crate::lsp::harness::TestHandlers;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;

/// Install each package with `DESCRIPTION` mtime 1000 seconds after the Unix
/// epoch. The tuple's boolean controls whether `data/Rdata.rds` exists.
fn library(packages: &[(&str, bool)]) -> (tempfile::TempDir, OakDatabase) {
    let lib = tempfile::tempdir().unwrap();
    for (name, rdata) in packages {
        install_package(lib.path(), name, 1_000, *rdata);
    }

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);
    (lib, db)
}

/// Each batch's packages with their requested `DESCRIPTION` revisions, in call
/// order.
fn batches(
    state: &GlobalState<TestHandlers>,
    sidecar: &TestSidecarHandler,
) -> Vec<Vec<(String, FileRevision)>> {
    let db = state.world().db();
    sidecar
        .calls()
        .lock()
        .unwrap()
        .iter()
        .map(|request| {
            request
                .packages()
                .iter()
                .map(|package| {
                    let name = package.package().name(db).clone();
                    (name, package.description_revision())
                })
                .collect()
        })
        .collect()
}

fn package(state: &GlobalState<TestHandlers>, name: &str) -> Package {
    state.world().db().package_by_name(name).unwrap()
}

fn revision(state: &GlobalState<TestHandlers>, name: &str) -> FileRevision {
    package(state, name).description_revision(state.world().db())
}

fn may_change(state: &GlobalState<TestHandlers>, name: &str) -> bool {
    let db = state.world().db();
    let package = db.package_by_name(name).unwrap();
    state.lsp_state().sidecar_scheduler.may_change(db, package)
}

fn wait_entered(entered_rx: &Receiver<()>) {
    entered_rx.recv_timeout(Duration::from_secs(30)).unwrap();
}

/// Open a new file so the tick ends with a refresh, which schedules a batch.
fn open_scratch(scratch: &Path, name: &str) -> Event<TestHandlers> {
    did_open(&scratch.join(name), "1\n")
}

/// Report a change to `dir`, a package directory, and apply the library
/// rescan it starts without handling a sidecar completion.
async fn rescan(state: &mut GlobalState<TestHandlers>, dir: &Path, typ: FileChangeType) {
    state
        .handle_event_once(did_change_watched_file(dir, typ))
        .await;
    state.pump_scans_to_quiescence().await;
}

/// Handle one sidecar completion without waiting for any batch it schedules.
async fn finish_batch(state: &mut GlobalState<TestHandlers>) {
    let completed = state
        .take_event(|event| matches!(event, Event::SidecarCompleted(_)))
        .await;
    state.handle_event_once(completed).await;
}

/// Packages that need facts while a batch runs wait for the next batch, which
/// the completion submits.
#[tokio::test]
async fn test_changes_during_batch_go_into_next_batch() {
    let _aux = init_aux_for_test();
    let (lib, db) = library(&[("base", false)]);

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (gate, release) = gate(entered_tx);
    let sidecar =
        Arc::new(TestSidecarHandler::gated(gate, vec![]).with_lazy_data("withdata", &["obj"]));
    let mut state = sidecar_state(db, Arc::clone(&sidecar));
    let scratch = tempfile::tempdir().unwrap();

    state
        .handle_event_once(open_scratch(scratch.path(), "a.R"))
        .await;
    wait_entered(&entered_rx);

    install_package(lib.path(), "withdata", 1_000, true);
    install_package(lib.path(), "nodata", 1_000, false);
    rescan(
        &mut state,
        &lib.path().join("withdata"),
        FileChangeType::CREATED,
    )
    .await;
    assert_eq!(batches(&state, &sidecar).len(), 1);

    // A package with no facts to compute is covered without waiting for the
    // batch in flight.
    let db = state.world().db();
    let nodata = package(&state, "nodata");
    assert_eq!(
        nodata.facts_revision(db),
        Some(nodata.description_revision(db))
    );
    assert!(!may_change(&state, "nodata"));
    assert!(may_change(&state, "withdata"));

    release.send(()).unwrap();
    finish_batch(&mut state).await;
    wait_entered(&entered_rx);

    assert_eq!(batches(&state, &sidecar), vec![
        vec![(String::from("base"), revision(&state, "base"))],
        vec![(String::from("withdata"), revision(&state, "withdata"))],
    ]);

    drop(release);
    state.pump_sidecar_to_quiescence().await;

    let db = state.world().db();
    assert_eq!(package(&state, "withdata").lazy_data(db), ["obj"]);
    assert!(!may_change(&state, "base"));
    assert!(!may_change(&state, "withdata"));
}

/// An upgrade during the batch makes its result stale. The result is dropped
/// and the new revision is requested.
#[tokio::test]
async fn test_completion_for_upgraded_package_is_dropped_and_requested_again() {
    let _aux = init_aux_for_test();
    let (lib, db) = library(&[("withdata", true)]);

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (gate, release) = gate(entered_tx);
    let sidecar =
        Arc::new(TestSidecarHandler::gated(gate, vec![]).with_lazy_data("withdata", &["old"]));
    let mut state = sidecar_state(db, Arc::clone(&sidecar));
    let scratch = tempfile::tempdir().unwrap();
    let old_revision = revision(&state, "withdata");

    state
        .handle_event_once(open_scratch(scratch.path(), "a.R"))
        .await;
    wait_entered(&entered_rx);

    install_package(lib.path(), "withdata", 2_000, true);
    sidecar.set_lazy_data("withdata", &["new"]);
    rescan(
        &mut state,
        &lib.path().join("withdata"),
        FileChangeType::CREATED,
    )
    .await;
    let new_revision = revision(&state, "withdata");
    assert_ne!(new_revision, old_revision);

    release.send(()).unwrap();
    finish_batch(&mut state).await;

    let withdata = package(&state, "withdata");
    let db = state.world().db();
    assert!(withdata.lazy_data(db).is_empty());
    assert_eq!(withdata.facts_revision(db), None);

    wait_entered(&entered_rx);
    assert_eq!(batches(&state, &sidecar), vec![
        vec![(String::from("withdata"), old_revision)],
        vec![(String::from("withdata"), new_revision)],
    ]);

    drop(release);
    state.pump_sidecar_to_quiescence().await;

    let db = state.world().db();
    assert_eq!(withdata.lazy_data(db), ["new"]);
    assert_eq!(withdata.facts_revision(db), Some(new_revision));
}

/// A package removed during the batch gets no facts. A reinstall with the
/// same `DESCRIPTION` revision revives the `Package`, which is requested again.
#[tokio::test]
async fn test_completion_for_removed_package_is_dropped_and_requested_again() {
    let _aux = init_aux_for_test();
    let (lib, db) = library(&[("withdata", true)]);

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (gate, release) = gate(entered_tx);
    let sidecar =
        Arc::new(TestSidecarHandler::gated(gate, vec![]).with_lazy_data("withdata", &["obj"]));
    let mut state = sidecar_state(db, Arc::clone(&sidecar));
    let scratch = tempfile::tempdir().unwrap();
    let withdata = package(&state, "withdata");
    let withdata_revision = revision(&state, "withdata");

    state
        .handle_event_once(open_scratch(scratch.path(), "a.R"))
        .await;
    wait_entered(&entered_rx);

    let dir = lib.path().join("withdata");
    std::fs::remove_dir_all(&dir).unwrap();
    rescan(&mut state, &dir, FileChangeType::DELETED).await;
    assert!(state.world().db().package_by_name("withdata").is_none());

    release.send(()).unwrap();
    finish_batch(&mut state).await;

    let db = state.world().db();
    assert!(withdata.lazy_data(db).is_empty());
    assert_eq!(withdata.facts_revision(db), None);
    assert!(!state.lsp_state().sidecar_scheduler.has_pending());

    install_package(lib.path(), "withdata", 1_000, true);
    rescan(&mut state, &dir, FileChangeType::CREATED).await;
    assert_eq!(package(&state, "withdata"), withdata);
    wait_entered(&entered_rx);

    assert_eq!(batches(&state, &sidecar), vec![
        vec![(String::from("withdata"), withdata_revision)],
        vec![(String::from("withdata"), withdata_revision)],
    ]);

    drop(release);
    state.pump_sidecar_to_quiescence().await;

    let db = state.world().db();
    assert_eq!(withdata.lazy_data(db), ["obj"]);
    assert_eq!(withdata.facts_revision(db), Some(withdata_revision));
}

/// A failed package settles without facts, isn't retried at the same
/// `DESCRIPTION` revision, and is retried after an upgrade.
#[tokio::test]
async fn test_failed_package_is_retried_only_at_new_revision() {
    let _aux = init_aux_for_test();
    let (lib, db) = library(&[("withdata", true)]);

    let sidecar = Arc::new(TestSidecarHandler::new(vec![]).with_failure("withdata"));
    let mut state = sidecar_state(db, Arc::clone(&sidecar));
    let scratch = tempfile::tempdir().unwrap();
    let old_revision = revision(&state, "withdata");

    state
        .handle_event_to_quiescence(open_scratch(scratch.path(), "a.R"))
        .await;

    let withdata = package(&state, "withdata");
    assert_eq!(withdata.facts_revision(state.world().db()), None);
    assert!(!may_change(&state, "withdata"));

    state
        .handle_event_to_quiescence(open_scratch(scratch.path(), "b.R"))
        .await;
    assert_eq!(batches(&state, &sidecar), vec![vec![(
        String::from("withdata"),
        old_revision
    )]]);

    install_package(lib.path(), "withdata", 2_000, true);
    state
        .handle_event_to_quiescence(did_change_watched_file(
            &lib.path().join("withdata"),
            FileChangeType::CREATED,
        ))
        .await;

    let new_revision = revision(&state, "withdata");
    assert_eq!(batches(&state, &sidecar), vec![
        vec![(String::from("withdata"), old_revision)],
        vec![(String::from("withdata"), new_revision)],
    ]);
    assert!(!may_change(&state, "withdata"));
}

/// A panicking handler sends no response. The pool's abort event reports the batch
/// as failed, so its packages settle instead of staying pending.
#[tokio::test]
async fn test_panicking_handler_is_reported_as_failed() {
    let _aux = init_aux_for_test();
    let (_lib, db) = library(&[("base", false), ("withdata", true)]);

    let sidecar = Arc::new(TestSidecarHandler::panicking());
    let mut state = sidecar_state(db, Arc::clone(&sidecar));
    let scratch = tempfile::tempdir().unwrap();

    state
        .handle_event_to_quiescence(open_scratch(scratch.path(), "a.R"))
        .await;

    assert!(!state.lsp_state().sidecar_scheduler.has_pending());
    assert!(!may_change(&state, "base"));
    assert!(!may_change(&state, "withdata"));

    state
        .handle_event_to_quiescence(open_scratch(scratch.path(), "b.R"))
        .await;
    assert_eq!(batches(&state, &sidecar), vec![vec![
        (String::from("base"), revision(&state, "base")),
        (String::from("withdata"), revision(&state, "withdata")),
    ]]);
}

/// `base`'s stub generation can fail while another package's lazy data read
/// succeeds. `base` settles as failed at its revision, and the other package
/// is still ingested.
#[tokio::test]
async fn test_failed_stubs_do_not_block_lazy_data_in_same_batch() {
    let _aux = init_aux_for_test();
    let (_lib, db) = library(&[("base", false), ("withdata", true)]);

    let sidecar = Arc::new(
        TestSidecarHandler::new(vec![("primitives.R", "`+` <- function(e1, e2) NULL\n")])
            .with_lazy_data("withdata", &["obj"])
            .with_stub_failure(),
    );
    let mut state = sidecar_state(db, Arc::clone(&sidecar));
    let scratch = tempfile::tempdir().unwrap();

    state
        .handle_event_to_quiescence(open_scratch(scratch.path(), "a.R"))
        .await;

    let db = state.world().db();
    let base = package(&state, "base");
    assert_eq!(base.facts_revision(db), None);
    assert!(base.files(db).is_empty());
    assert!(!may_change(&state, "base"));

    let withdata = package(&state, "withdata");
    assert_eq!(
        withdata.facts_revision(db),
        Some(withdata.description_revision(db))
    );
    assert_eq!(withdata.lazy_data(db), ["obj"]);
    assert!(!may_change(&state, "withdata"));

    state
        .handle_event_to_quiescence(open_scratch(scratch.path(), "b.R"))
        .await;
    assert_eq!(batches(&state, &sidecar), vec![vec![
        (String::from("base"), revision(&state, "base")),
        (String::from("withdata"), revision(&state, "withdata")),
    ]]);
}

/// A `base` without a `Built:` field can't compute a primitive-stub cache key,
/// and with no `data/Rdata.rds` it has nothing else to request. It is covered
/// right away and never reaches a batch.
#[tokio::test]
async fn test_base_without_built_field_is_covered_without_a_batch() {
    let _aux = init_aux_for_test();
    let lib = tempfile::tempdir().unwrap();
    DescriptionWriter::new()
        .package("base")
        .version("0.0.0")
        .field("Priority", "base")
        .write_installed(&lib.path().join("base"));

    let mut db = OakDatabase::new();
    db.set_library_paths(&[lib.path().to_path_buf()]);

    let sidecar = Arc::new(TestSidecarHandler::new(vec![]));
    let mut state = sidecar_state(db, Arc::clone(&sidecar));
    let scratch = tempfile::tempdir().unwrap();

    state
        .handle_event_to_quiescence(open_scratch(scratch.path(), "a.R"))
        .await;

    let base = package(&state, "base");
    let db = state.world().db();
    assert_eq!(base.built(db), None);
    assert_eq!(base.facts_revision(db), Some(base.description_revision(db)));
    assert!(base.files(db).is_empty());
    assert!(!may_change(&state, "base"));
    assert!(batches(&state, &sidecar).is_empty());
}
