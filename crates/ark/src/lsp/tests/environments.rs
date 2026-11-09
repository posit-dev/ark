//! Main-loop integration tests for environment resolution and library watcher
//! replacement. The merge and eviction writers are unit tested in `oak_scan`.

use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use aether_path::FilePath;
use oak_db::DbInputs;
use oak_db::Environment;
use oak_db::EnvironmentStatus;
use serde_json::Value;
use tempfile::TempDir;
use tokio::sync::mpsc::UnboundedReceiver;
use tower_lsp_server::ls_types::ClientCapabilities;
use tower_lsp_server::ls_types::Diagnostic;
use tower_lsp_server::ls_types::DiagnosticSeverity;
use tower_lsp_server::ls_types::DidChangeWatchedFilesClientCapabilities;
use tower_lsp_server::ls_types::DidChangeWatchedFilesParams;
use tower_lsp_server::ls_types::DidChangeWatchedFilesRegistrationOptions;
use tower_lsp_server::ls_types::DidChangeWorkspaceFoldersParams;
use tower_lsp_server::ls_types::FileChangeType;
use tower_lsp_server::ls_types::FileEvent;
use tower_lsp_server::ls_types::GlobPattern;
use tower_lsp_server::ls_types::InitializeParams;
use tower_lsp_server::ls_types::OneOf;
use tower_lsp_server::ls_types::Position;
use tower_lsp_server::ls_types::RegistrationParams;
use tower_lsp_server::ls_types::UnregistrationParams;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::ls_types::WorkspaceClientCapabilities;
use tower_lsp_server::ls_types::WorkspaceFolder;
use tower_lsp_server::ls_types::WorkspaceFoldersChangeEvent;

use super::environment_resolver::TestBehavior;
use super::environment_resolver::TestEnvironmentResolver;
use super::source_handler::gate;
use super::source_handler::wait_for_entries;
use super::utils::did_change_workspace_folders;
use super::utils::did_open;
use super::utils::initialize;
use super::utils::initialized;
use super::utils::test_client;
use super::utils::TestClient;
use crate::lsp::backend::LspMessage;
use crate::lsp::backend::LspNotification;
use crate::lsp::backend::LspRequest;
use crate::lsp::backend::RequestResponse;
use crate::lsp::diagnostics::generate_diagnostics;
use crate::lsp::diagnostics::WORKSPACE_ENVIRONMENT_FIXES;
use crate::lsp::environments::unresolved_database;
use crate::lsp::environments::EnvironmentRequest;
use crate::lsp::environments::EnvironmentResolver;
use crate::lsp::environments::EnvironmentScheduler;
use crate::lsp::environments::NO_R_EXECUTABLE;
use crate::lsp::main_loop::init_aux_for_test;
use crate::lsp::main_loop::AuxiliaryEvent;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::GlobalState;
use crate::lsp::main_loop::LspState;
use crate::lsp::state::WorldState;

/// The production constructor starts the default unresolved, so a failed
/// first run reads as a first failure rather than a failed rerun. An `r_home`
/// without R is enough, since construction runs nothing.
#[tokio::test]
async fn test_global_state_starts_with_default_unresolved() {
    let _aux = init_aux_for_test();
    let r_home = tempfile::tempdir().unwrap();
    let (console_tx, _console_rx) = tokio::sync::mpsc::unbounded_channel();

    let state = GlobalState::new(test_client(), r_home.path().to_path_buf(), console_tx);

    let db = state.world().db();
    assert_eq!(
        db.default_environment().status(db),
        &EnvironmentStatus::Unresolved
    );
}

/// Initial resolution requests the default before the root, and the merged
/// list puts workspace environments first, listing a shared library once.
#[tokio::test]
async fn test_initial_resolution_merges_root_and_default_libraries() {
    let _aux = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();
    let [root_lib, shared_lib, default_lib] = libraries();

    let resolver = Arc::new(TestEnvironmentResolver::new([
        (
            EnvironmentRequest::Default,
            TestBehavior::Success(vec![path(&default_lib), path(&shared_lib)]),
        ),
        (
            directory(workspace.path()),
            TestBehavior::Success(vec![path(&root_lib), path(&shared_lib)]),
        ),
    ]));
    let mut state = global_state(Some(resolver.clone()));

    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;

    assert_eq!(resolver.calls(), vec![
        EnvironmentRequest::Default,
        directory(workspace.path())
    ]);
    assert_eq!(library_paths(&state), vec![
        path(&root_lib),
        path(&shared_lib),
        path(&default_lib)
    ]);
    assert_eq!(workspace_environment_dirs(&state), vec![workspace
        .path()
        .to_path_buf()]);

    let default = state.world().db().default_environment();
    assert_eq!(
        resolved_paths(&state, default),
        Some(vec![path(&default_lib), path(&shared_lib)])
    );
}

/// A scan that discovers a nested `.Rprofile` adds an environment for its
/// directory and resolves it after the root.
#[tokio::test]
async fn test_scan_discovering_nested_rprofile_resolves_its_environment() {
    let _aux = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();
    let sub = workspace.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join(".Rprofile"), "").unwrap();
    let [root_lib, sub_lib, _] = libraries();

    let resolver = Arc::new(TestEnvironmentResolver::new([
        (EnvironmentRequest::Default, TestBehavior::Success(vec![])),
        (
            directory(workspace.path()),
            TestBehavior::Success(vec![path(&root_lib)]),
        ),
        (directory(&sub), TestBehavior::Success(vec![path(&sub_lib)])),
    ]));
    let mut state = global_state(Some(resolver.clone()));

    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;

    assert_eq!(resolver.calls(), vec![
        EnvironmentRequest::Default,
        directory(workspace.path()),
        directory(&sub)
    ]);
    assert_eq!(workspace_environment_dirs(&state), vec![
        workspace.path().to_path_buf(),
        sub.clone()
    ]);
    assert_eq!(library_paths(&state), vec![path(&root_lib), path(&sub_lib)]);
}

#[tokio::test]
async fn test_removing_folder_drops_environment_and_its_libraries() {
    let _aux = init_aux_for_test();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let [first_lib, second_lib, _] = libraries();

    let resolver = Arc::new(TestEnvironmentResolver::new([
        (EnvironmentRequest::Default, TestBehavior::Success(vec![])),
        (
            directory(first.path()),
            TestBehavior::Success(vec![path(&first_lib)]),
        ),
        (
            directory(second.path()),
            TestBehavior::Success(vec![path(&second_lib)]),
        ),
    ]));
    let mut state = global_state(Some(resolver.clone()));

    let (event, _response_rx) = initialize(first.path());
    state.handle_event_to_quiescence(event).await;
    state
        .handle_event_to_quiescence(did_change_workspace_folders(second.path()))
        .await;
    assert_eq!(library_paths(&state), vec![
        path(&first_lib),
        path(&second_lib)
    ]);

    state
        .handle_event_to_quiescence(folder_removed(second.path()))
        .await;

    assert_eq!(workspace_environment_dirs(&state), vec![first
        .path()
        .to_path_buf()]);
    assert_eq!(library_paths(&state), vec![path(&first_lib)]);
}

/// A completion for an environment removed while its job ran is dropped, and
/// a re-run queued by a sentinel edit during the job is not started.
#[tokio::test]
async fn test_late_completion_for_removed_environment_is_dropped() {
    let _aux = init_aux_for_test();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    fs::write(second.path().join(".Rprofile"), "").unwrap();
    let [first_lib, second_lib, _] = libraries();

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (second_gate, release) = gate(entered_tx);
    let resolver = Arc::new(TestEnvironmentResolver::new([
        (EnvironmentRequest::Default, TestBehavior::Success(vec![])),
        (
            directory(first.path()),
            TestBehavior::Success(vec![path(&first_lib)]),
        ),
        (
            directory(second.path()),
            TestBehavior::Gated(Arc::new(second_gate), vec![path(&second_lib)]),
        ),
    ]));
    let mut state = global_state(Some(resolver.clone()));

    let (event, _response_rx) = initialize(first.path());
    state.handle_event_to_quiescence(event).await;

    state
        .handle_event_once(did_change_workspace_folders(second.path()))
        .await;
    wait_for_entries(&entered_rx, 1);

    state
        .handle_event_once(sentinel_changed(&[&second.path().join(".Rprofile")]))
        .await;
    state.handle_event_once(folder_removed(second.path())).await;

    drop(release);
    state.pump_to_quiescence().await;

    assert_eq!(resolver.call_count(&directory(second.path())), 1);
    assert_eq!(workspace_environment_dirs(&state), vec![first
        .path()
        .to_path_buf()]);
    assert_eq!(library_paths(&state), vec![path(&first_lib)]);
}

/// A sentinel edit re-runs only the environment whose directory holds it. The
/// previous libraries stay in place until the new result lands.
#[tokio::test]
async fn test_sentinel_edit_reruns_its_environment_and_keeps_libraries_until_result() {
    let _aux = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();
    let sub = workspace.path().join("sub");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join(".Rprofile"), "").unwrap();
    let [root_lib, old_lib, new_lib] = libraries();

    let resolver = Arc::new(TestEnvironmentResolver::new([
        (EnvironmentRequest::Default, TestBehavior::Success(vec![])),
        (
            directory(workspace.path()),
            TestBehavior::Success(vec![path(&root_lib)]),
        ),
        (directory(&sub), TestBehavior::Success(vec![path(&old_lib)])),
    ]));
    let mut state = global_state(Some(resolver.clone()));

    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;
    assert_eq!(library_paths(&state), vec![path(&root_lib), path(&old_lib)]);

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (sub_gate, release) = gate(entered_tx);
    resolver.set(
        directory(&sub),
        TestBehavior::Gated(Arc::new(sub_gate), vec![path(&new_lib)]),
    );

    // An R file in the root is in the same batch but re-runs nothing.
    state
        .handle_event_once(sentinel_changed(&[
            &sub.join(".Rprofile"),
            &workspace.path().join("a.R"),
        ]))
        .await;
    wait_for_entries(&entered_rx, 1);

    let sub_environment = workspace_environment(&state, &sub);
    assert_eq!(
        resolved_paths(&state, sub_environment),
        Some(vec![path(&old_lib)])
    );
    assert_eq!(library_paths(&state), vec![path(&root_lib), path(&old_lib)]);

    drop(release);
    state.pump_to_quiescence().await;

    assert_eq!(resolver.call_count(&directory(workspace.path())), 1);
    assert_eq!(resolver.call_count(&directory(&sub)), 2);
    assert_eq!(library_paths(&state), vec![path(&root_lib), path(&new_lib)]);
}

/// Two edits while a job runs lead to exactly one more run, and the result of
/// the superseded run is never applied.
#[tokio::test]
async fn test_edits_during_job_rerun_once_and_drop_superseded_result() {
    let _aux = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();
    let rprofile = workspace.path().join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let [first_lib, superseded_lib, final_lib] = libraries();

    let resolver = Arc::new(TestEnvironmentResolver::new([
        (EnvironmentRequest::Default, TestBehavior::Success(vec![])),
        (
            directory(workspace.path()),
            TestBehavior::Success(vec![path(&first_lib)]),
        ),
    ]));
    let mut state = global_state(Some(resolver.clone()));

    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (workspace_gate, release) = gate(entered_tx);
    resolver.set(
        directory(workspace.path()),
        TestBehavior::Gated(Arc::new(workspace_gate), vec![path(&superseded_lib)]),
    );
    state
        .handle_event_once(sentinel_changed(&[&rprofile]))
        .await;
    wait_for_entries(&entered_rx, 1);

    resolver.set(
        directory(workspace.path()),
        TestBehavior::Success(vec![path(&final_lib)]),
    );
    state
        .handle_event_once(sentinel_changed(&[&rprofile]))
        .await;
    state
        .handle_event_once(sentinel_changed(&[&rprofile]))
        .await;

    drop(release);
    let superseded = state.take_event(is_environment_resolved).await;
    state.handle_event_once(superseded).await;
    assert_eq!(library_paths(&state), vec![path(&first_lib)]);

    state.pump_to_quiescence().await;

    assert_eq!(resolver.call_count(&directory(workspace.path())), 3);
    assert_eq!(library_paths(&state), vec![path(&final_lib)]);
}

#[tokio::test]
async fn test_first_failure_leaves_environment_unresolved() {
    let _aux = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();
    let [default_lib, _, _] = libraries();

    let resolver = Arc::new(TestEnvironmentResolver::new([
        (
            EnvironmentRequest::Default,
            TestBehavior::Success(vec![path(&default_lib)]),
        ),
        (
            directory(workspace.path()),
            TestBehavior::Failure("R exited with status 1"),
        ),
    ]));
    let mut state = global_state(Some(resolver));

    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;

    let environment = workspace_environment(&state, workspace.path());
    assert_eq!(resolved_paths(&state, environment), None);
    assert_eq!(
        last_error(&state, environment),
        Some(String::from("R exited with status 1"))
    );
    assert_eq!(library_paths(&state), vec![path(&default_lib)]);
}

#[tokio::test]
async fn test_failed_rerun_keeps_previous_libraries() {
    let _aux = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();
    let rprofile = workspace.path().join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let [lib, _, _] = libraries();

    let resolver = Arc::new(TestEnvironmentResolver::new([
        (EnvironmentRequest::Default, TestBehavior::Success(vec![])),
        (
            directory(workspace.path()),
            TestBehavior::Success(vec![path(&lib)]),
        ),
    ]));
    let mut state = global_state(Some(resolver.clone()));

    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;

    resolver.set(
        directory(workspace.path()),
        TestBehavior::Failure("R timed out after 30 s"),
    );
    state
        .handle_event_to_quiescence(sentinel_changed(&[&rprofile]))
        .await;

    let environment = workspace_environment(&state, workspace.path());
    assert_eq!(resolved_paths(&state, environment), Some(vec![path(&lib)]));
    assert_eq!(
        last_error(&state, environment),
        Some(String::from("R timed out after 30 s"))
    );
    assert_eq!(library_paths(&state), vec![path(&lib)]);
}

/// The production database starts with the default unresolved
/// (`unresolved_database()`), and `initialize` only requests its run, so a
/// failed first run leaves it without libraries rather than resolved to none.
#[tokio::test]
async fn test_failed_default_stays_unresolved() {
    let _aux = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();
    let [lib, _, _] = libraries();

    let resolver = Arc::new(TestEnvironmentResolver::new([
        (
            EnvironmentRequest::Default,
            TestBehavior::Failure("R exited with status 1"),
        ),
        (
            directory(workspace.path()),
            TestBehavior::Success(vec![path(&lib)]),
        ),
    ]));
    let mut state = global_state(Some(resolver));

    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;

    let default = state.world().db().default_environment();
    assert_eq!(resolved_paths(&state, default), None);
    assert_eq!(
        last_error(&state, default),
        Some(String::from("R exited with status 1"))
    );
    assert_eq!(library_paths(&state), vec![path(&lib)]);
}

/// Jobs without a resolver report the same failure for every environment
/// without starting R.
#[tokio::test]
async fn test_missing_r_fails_every_environment() {
    let _aux = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();
    let mut state = global_state(None);

    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;

    let default = state.world().db().default_environment();
    let root = workspace_environment(&state, workspace.path());
    for environment in [default, root] {
        assert_eq!(resolved_paths(&state, environment), None);
        assert_eq!(
            last_error(&state, environment),
            Some(String::from(NO_R_EXECUTABLE))
        );
    }
    assert_eq!(library_paths(&state), Vec::<PathBuf>::new());
}

/// The job guard reports a generic failure after a resolver panic, allowing
/// the scheduler to settle.
#[tokio::test]
async fn test_resolver_panic_records_failure() {
    let _aux = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();

    let resolver = Arc::new(TestEnvironmentResolver::new([
        (EnvironmentRequest::Default, TestBehavior::Success(vec![])),
        (directory(workspace.path()), TestBehavior::Panic),
    ]));
    let mut state = global_state(Some(resolver));

    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;

    let environment = workspace_environment(&state, workspace.path());
    assert_eq!(resolved_paths(&state, environment), None);
    assert_eq!(
        last_error(&state, environment),
        Some(String::from(
            "the library resolution job stopped before reporting a result"
        ))
    );
}

/// A result that changes the merged list after `initialized` registers the
/// new list, then unregisters the stored library watcher registration.
#[tokio::test]
async fn test_library_change_replaces_watcher_registration() {
    let _aux = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();
    let rprofile = workspace.path().join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let [old_lib, new_lib, default_lib] = libraries();

    let resolver = Arc::new(TestEnvironmentResolver::new([
        (
            EnvironmentRequest::Default,
            TestBehavior::Success(vec![path(&default_lib)]),
        ),
        (
            directory(workspace.path()),
            TestBehavior::Success(vec![path(&old_lib)]),
        ),
    ]));
    let client = TestClient::new(&[]).await;
    let mut state = GlobalState::from_parts(
        client.client(),
        WorldState::new(unresolved_database()),
        lsp_state(Some(resolver.clone())),
    );

    let (event, _response_rx) = initialize_with_relative_patterns(workspace.path());
    state.handle_event_to_quiescence(event).await;
    state.handle_event_to_quiescence(initialized()).await;

    let requests = client.request_params();
    let [(method, params)] = requests.as_slice() else {
        panic!("Expected one request at `initialized`, got {requests:?}");
    };
    assert_eq!(method, "client/registerCapability");
    let [(initial_id, initial_watched)] = library_registrations(params).try_into().unwrap();
    assert_eq!(initial_watched, vec![uri(&old_lib), uri(&default_lib)]);

    resolver.set(
        directory(workspace.path()),
        TestBehavior::Success(vec![path(&new_lib)]),
    );
    state
        .handle_event_to_quiescence(sentinel_changed(&[&rprofile]))
        .await;

    let requests = client.request_params();
    let [_, (register_method, register_params), (unregister_method, unregister_params)] =
        requests.as_slice()
    else {
        panic!("Expected a registration and an unregistration, got {requests:?}");
    };

    assert_eq!(unregister_method, "client/unregisterCapability");
    let unregistrations: UnregistrationParams =
        serde_json::from_value(unregister_params.clone()).unwrap();
    let ids: Vec<&str> = unregistrations
        .unregisterations
        .iter()
        .map(|unregistration| unregistration.id.as_str())
        .collect();
    assert_eq!(ids, vec![initial_id.as_str()]);

    assert_eq!(register_method, "client/registerCapability");
    let [(new_id, new_watched)] = library_registrations(register_params).try_into().unwrap();
    assert_ne!(new_id, initial_id);
    assert_eq!(new_watched, vec![uri(&new_lib), uri(&default_lib)]);
}

/// Clients send `initialized` while R is still starting, so `initialized`
/// registers no library watchers. The first library registration comes from a
/// later result, with nothing to unregister, and an empty list only
/// unregisters.
#[tokio::test]
async fn test_library_watchers_follow_results_after_initialized() {
    let _aux = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();
    let rprofile = workspace.path().join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let [root_lib, _, _] = libraries();

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (default_gate, release) = gate(entered_tx);
    let resolver = Arc::new(TestEnvironmentResolver::new([
        (
            EnvironmentRequest::Default,
            TestBehavior::Gated(Arc::new(default_gate), vec![]),
        ),
        (
            directory(workspace.path()),
            TestBehavior::Success(vec![path(&root_lib)]),
        ),
    ]));
    let client = TestClient::new(&[]).await;
    let mut state = GlobalState::from_parts(
        client.client(),
        WorldState::new(unresolved_database()),
        lsp_state(Some(resolver.clone())),
    );

    let (event, _response_rx) = initialize_with_relative_patterns(workspace.path());
    state.handle_event_once(event).await;
    wait_for_entries(&entered_rx, 1);
    state.handle_event_once(initialized()).await;

    let requests = client.request_params();
    let [(method, params)] = requests.as_slice() else {
        panic!("Expected one request at `initialized`, got {requests:?}");
    };
    assert_eq!(method, "client/registerCapability");
    assert_eq!(library_registrations(params), Vec::new());

    drop(release);
    state.pump_to_quiescence().await;

    let requests = client.request_params();
    let [_, (register_method, register_params)] = requests.as_slice() else {
        panic!("Expected one registration after resolution, got {requests:?}");
    };
    assert_eq!(register_method, "client/registerCapability");
    let [(id, watched)] = library_registrations(register_params).try_into().unwrap();
    assert_eq!(watched, vec![uri(&root_lib)]);

    resolver.set(directory(workspace.path()), TestBehavior::Success(vec![]));
    state
        .handle_event_to_quiescence(sentinel_changed(&[&rprofile]))
        .await;

    let requests = client.request_params();
    let [_, _, (unregister_method, unregister_params)] = requests.as_slice() else {
        panic!("Expected one unregistration after the list emptied, got {requests:?}");
    };
    assert_eq!(unregister_method, "client/unregisterCapability");
    let unregistrations: UnregistrationParams =
        serde_json::from_value(unregister_params.clone()).unwrap();
    let ids: Vec<&str> = unregistrations
        .unregisterations
        .iter()
        .map(|unregistration| unregistration.id.as_str())
        .collect();
    assert_eq!(ids, vec![id.as_str()]);
}

/// A first startup failure suppresses semantic diagnostics. After a success,
/// another failure retains semantic diagnostics and adds a warning. Success
/// clears either failure diagnostic.
#[tokio::test]
async fn test_open_file_diagnostics_follow_environment_failures() {
    let _aux = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();
    let rprofile = workspace.path().join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let script = workspace.path().join("a.R");
    let code = "foo\n1 }\n";
    fs::write(&script, code).unwrap();

    let resolver = Arc::new(TestEnvironmentResolver::new([
        (EnvironmentRequest::Default, TestBehavior::Success(vec![])),
        (
            directory(workspace.path()),
            TestBehavior::Failure("R exited with status 1"),
        ),
    ]));
    let mut state = global_state(Some(resolver.clone()));

    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_to_quiescence(event).await;
    state
        .handle_event_to_quiescence(did_open(&script, code))
        .await;

    let dir = workspace.path().display();
    let rprofile_shown = rprofile.display();
    let diagnostics = open_file_diagnostics(&state, &script);
    let [failure, syntax] = diagnostics.as_slice() else {
        panic!("Expected two diagnostics, got {diagnostics:?}");
    };
    assert_eq!(failure.severity, Some(DiagnosticSeverity::INFORMATION));
    assert_eq!(
        failure.message,
        format!(
            "R startup failed in {dir}, so only syntax is checked. See {rprofile_shown}.\n\
             \n\
             R exited with status 1\n\
             \n\
             {WORKSPACE_ENVIRONMENT_FIXES}"
        )
    );
    let full = vec![
        syntax.message.clone(),
        String::from("No symbol named 'foo' in scope."),
    ];

    resolver.set(directory(workspace.path()), TestBehavior::Success(vec![]));
    state
        .handle_event_to_quiescence(sentinel_changed(&[&rprofile]))
        .await;
    assert_eq!(open_file_diagnostic_messages(&state, &script), full);

    resolver.set(
        directory(workspace.path()),
        TestBehavior::Failure("R timed out"),
    );
    state
        .handle_event_to_quiescence(sentinel_changed(&[&rprofile]))
        .await;

    let diagnostics = open_file_diagnostics(&state, &script);
    let [failure, rest @ ..] = diagnostics.as_slice() else {
        panic!("Expected diagnostics");
    };
    assert_eq!(failure.severity, Some(DiagnosticSeverity::WARNING));
    assert_eq!(
        failure.message,
        format!(
            "R startup failed in {dir}, so libraries come from the last successful startup. \
             See {rprofile_shown}.\n\
             \n\
             R timed out\n\
             \n\
             {WORKSPACE_ENVIRONMENT_FIXES}"
        )
    );
    let rest: Vec<String> = rest
        .iter()
        .map(|diagnostic| diagnostic.message.clone())
        .collect();
    assert_eq!(rest, full);

    resolver.set(directory(workspace.path()), TestBehavior::Success(vec![]));
    state
        .handle_event_to_quiescence(sentinel_changed(&[&rprofile]))
        .await;
    assert_eq!(open_file_diagnostic_messages(&state, &script), full);
}

/// What the main loop publishes for an open file follows its environment
/// without a document edit: syntax only while the first run is in flight, the
/// startup failure once it fails, and full diagnostics once a rerun succeeds.
#[tokio::test]
async fn test_published_diagnostics_follow_environment_resolution() {
    let mut aux_rx = init_aux_for_test();
    let workspace = tempfile::tempdir().unwrap();
    let rprofile = workspace.path().join(".Rprofile");
    fs::write(&rprofile, "").unwrap();
    let script = workspace.path().join("a.R");
    let code = "foo\n1 }\n";
    fs::write(&script, code).unwrap();

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (workspace_gate, release) = gate(entered_tx);
    let resolver = Arc::new(TestEnvironmentResolver::new([
        (EnvironmentRequest::Default, TestBehavior::Success(vec![])),
        (
            directory(workspace.path()),
            TestBehavior::GatedFailure(Arc::new(workspace_gate), "R exited with status 1"),
        ),
    ]));
    let mut state = global_state(Some(resolver.clone()));

    // Scanning places `a.R` in the workspace root, so it uses the workspace
    // environment, whose run is then held at the gate.
    let (event, _response_rx) = initialize(workspace.path());
    state.handle_event_once(event).await;
    state.pump_scans_to_quiescence().await;
    wait_for_entries(&entered_rx, 1);

    state.handle_event_once(did_open(&script, code)).await;
    let published = pump_publications(&mut state, &mut aux_rx, &script, |_state, published| {
        !published.is_empty()
    })
    .await;
    let [syntax] = published[0].as_slice() else {
        panic!("Expected one diagnostic, got {published:?}");
    };
    // The syntax error at the unmatched `}`.
    assert_eq!(syntax.range.start, Position::new(1, 2));

    drop(release);
    let published = pump_publications(&mut state, &mut aux_rx, &script, |state, _published| {
        state.is_settled()
    })
    .await;
    let Some(last) = published.last() else {
        panic!("Expected a publication after the failed run");
    };
    let [failure, after] = last.as_slice() else {
        panic!("Expected two diagnostics, got {last:?}");
    };
    assert_eq!(failure.severity, Some(DiagnosticSeverity::INFORMATION));
    assert_eq!(
        failure.message,
        format!(
            "R startup failed in {dir}, so only syntax is checked. See {rprofile}.\n\
             \n\
             R exited with status 1\n\
             \n\
             {WORKSPACE_ENVIRONMENT_FIXES}",
            dir = workspace.path().display(),
            rprofile = rprofile.display(),
        )
    );
    assert_eq!(after, syntax);

    resolver.set(directory(workspace.path()), TestBehavior::Success(vec![]));
    state
        .handle_event_once(sentinel_changed(&[&rprofile]))
        .await;
    let published = pump_publications(&mut state, &mut aux_rx, &script, |state, _published| {
        state.is_settled()
    })
    .await;
    let Some(last) = published.last() else {
        panic!("Expected a publication after the successful run");
    };
    let messages: Vec<&str> = last
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect();
    assert_eq!(messages, vec![
        syntax.message.as_str(),
        "No symbol named 'foo' in scope."
    ]);
}

/// Generate diagnostics for the open buffer directly, without waiting for
/// background publication.
fn open_file_diagnostics(state: &GlobalState, path: &Path) -> Vec<Diagnostic> {
    let world = state.world();
    let file_path = FilePath::from_path_buf(path.to_path_buf()).unwrap();
    let open_file = world.open_file(&file_path).unwrap();
    generate_diagnostics(
        open_file.file(),
        world.snapshot(),
        false,
        open_file.wire_uri(),
    )
}

fn open_file_diagnostic_messages(state: &GlobalState, path: &Path) -> Vec<String> {
    open_file_diagnostics(state, path)
        .into_iter()
        .map(|diagnostic| diagnostic.message)
        .collect()
}

/// Pump the main loop until `done` holds, returning the diagnostics published
/// for `path` meanwhile, oldest first. Publications come from the auxiliary
/// channel, so they have passed refresh scheduling and generation filtering.
///
/// The analysis pool can go idle without sending an event, so `done` is
/// re-checked every millisecond rather than only after events.
async fn pump_publications(
    state: &mut GlobalState,
    aux_rx: &mut UnboundedReceiver<AuxiliaryEvent>,
    path: &Path,
    done: impl Fn(&GlobalState, &[Vec<Diagnostic>]) -> bool,
) -> Vec<Vec<Diagnostic>> {
    const POLL: Duration = Duration::from_millis(1);
    const TIMEOUT: Duration = Duration::from_secs(30);

    let file_path = FilePath::from_path_buf(path.to_path_buf()).unwrap();
    let deadline = Instant::now() + TIMEOUT;
    let mut published = Vec::new();
    loop {
        while let Ok(event) = aux_rx.try_recv() {
            let AuxiliaryEvent::PublishDiagnostics(publication) = event else {
                continue;
            };
            if publication.path == file_path {
                published.push(publication.diagnostics);
            }
        }
        if done(state, &published) {
            return published;
        }
        if Instant::now() > deadline {
            panic!(
                "No expected diagnostics for {path} within {TIMEOUT:?}, got {published:?}",
                path = path.display()
            );
        }
        if let Ok(event) = tokio::time::timeout(POLL, state.next_event()).await {
            state.handle_event_once(event).await;
        }
    }
}

fn global_state(resolver: Option<Arc<TestEnvironmentResolver>>) -> GlobalState {
    GlobalState::from_parts(
        test_client(),
        WorldState::new(unresolved_database()),
        lsp_state(resolver),
    )
}

fn lsp_state(resolver: Option<Arc<TestEnvironmentResolver>>) -> LspState {
    let resolver = resolver.map(|resolver| resolver as Arc<dyn EnvironmentResolver>);
    LspState::new(tokio::sync::mpsc::unbounded_channel().0)
        .with_environment_scheduler(EnvironmentScheduler::new(resolver))
}

/// Empty directories suffice because these tests check library paths, not
/// installed packages.
fn libraries() -> [TempDir; 3] {
    [(); 3].map(|()| tempfile::tempdir().unwrap())
}

fn path(dir: &TempDir) -> PathBuf {
    dir.path().to_path_buf()
}

fn uri(dir: &TempDir) -> Uri {
    Uri::from_file_path(dir.path()).unwrap()
}

fn directory(dir: &Path) -> EnvironmentRequest {
    EnvironmentRequest::Directory(dir.to_path_buf())
}

fn library_paths(state: &GlobalState) -> Vec<PathBuf> {
    let db = state.world().db();
    db.library_roots()
        .roots(db)
        .iter()
        .map(|root| root.path(db).as_path().unwrap().as_std_path().to_path_buf())
        .collect()
}

fn workspace_environment_dirs(state: &GlobalState) -> Vec<PathBuf> {
    let db = state.world().db();
    db.workspace_environments()
        .environments(db)
        .iter()
        .map(|environment| {
            let dir = environment.dir(db).as_ref().unwrap();
            dir.as_path().unwrap().as_std_path().to_path_buf()
        })
        .collect()
}

fn workspace_environment(state: &GlobalState, dir: &Path) -> Environment {
    let db = state.world().db();
    let found = db
        .workspace_environments()
        .environments(db)
        .iter()
        .copied()
        .find(|environment| {
            environment
                .dir(db)
                .as_ref()
                .and_then(|env_dir| env_dir.as_path())
                .is_some_and(|env_dir| env_dir.as_std_path() == dir)
        });
    match found {
        Some(environment) => environment,
        None => panic!("No live environment for {}", dir.display()),
    }
}

fn resolved_paths(state: &GlobalState, environment: Environment) -> Option<Vec<PathBuf>> {
    let db = state.world().db();
    match environment.status(db) {
        EnvironmentStatus::Unresolved => None,
        EnvironmentStatus::Resolved(roots) => Some(
            roots
                .iter()
                .map(|root| root.path(db).as_path().unwrap().as_std_path().to_path_buf())
                .collect(),
        ),
    }
}

fn last_error(state: &GlobalState, environment: Environment) -> Option<String> {
    let db = state.world().db();
    environment.last_error(db).clone()
}

fn is_environment_resolved(event: &Event) -> bool {
    matches!(event, Event::EnvironmentResolved(_))
}

fn sentinel_changed(paths: &[&Path]) -> Event {
    let changes = paths
        .iter()
        .map(|path| FileEvent {
            uri: Uri::from_file_path(path).unwrap(),
            typ: FileChangeType::CHANGED,
        })
        .collect();
    Event::Lsp(LspMessage::Notification(
        LspNotification::DidChangeWatchedFiles(DidChangeWatchedFilesParams { changes }),
    ))
}

fn folder_removed(path: &Path) -> Event {
    Event::Lsp(LspMessage::Notification(
        LspNotification::DidChangeWorkspaceFolders(DidChangeWorkspaceFoldersParams {
            event: WorkspaceFoldersChangeEvent {
                added: vec![],
                removed: vec![WorkspaceFolder {
                    uri: Uri::from_file_path(path).unwrap(),
                    name: String::new(),
                }],
            },
        }),
    ))
}

/// An `initialize` request from a client that can watch libraries outside the
/// workspace through relative patterns.
fn initialize_with_relative_patterns(path: &Path) -> (Event, UnboundedReceiver<RequestResponse>) {
    let params = InitializeParams {
        capabilities: ClientCapabilities {
            workspace: Some(WorkspaceClientCapabilities {
                did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                    relative_pattern_support: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        },
        workspace_folders: Some(vec![WorkspaceFolder {
            uri: Uri::from_file_path(path).unwrap(),
            name: String::new(),
        }]),
        ..Default::default()
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let event = Event::Lsp(LspMessage::Request(LspRequest::Initialize(params), tx));
    (event, rx)
}

/// The id and watched base URIs of each library watcher registration in a
/// `client/registerCapability` request. Library watchers are the ones with
/// relative patterns.
fn library_registrations(params: &Value) -> Vec<(String, Vec<Uri>)> {
    let params: RegistrationParams = serde_json::from_value(params.clone()).unwrap();
    params
        .registrations
        .into_iter()
        .filter_map(|registration| {
            let options: DidChangeWatchedFilesRegistrationOptions =
                serde_json::from_value(registration.register_options?).ok()?;
            let watched: Vec<Uri> = options
                .watchers
                .into_iter()
                .filter_map(|watcher| match watcher.glob_pattern {
                    GlobPattern::Relative(pattern) => match pattern.base_uri {
                        OneOf::Right(uri) => Some(uri),
                        OneOf::Left(folder) => Some(folder.uri),
                    },
                    GlobPattern::String(_) => None,
                })
                .collect();
            if watched.is_empty() {
                return None;
            }
            Some((registration.id, watched))
        })
        .collect()
}
