//
// kernel-hooks-session.rs
//
// Copyright (C) 2026 by Posit Software, PBC
//
//

use amalthea::comm::ui_comm::UiBackendReply;
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

/// Helper to execute R code and consume the full busy/idle window including
/// UI comm busy/prompt_state events.
fn execute(frontend: &DummyArkFrontend, comm_id: &str, code: &str) {
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_ui_busy(comm_id, true);
    frontend.recv_ui_busy(comm_id, false);
    frontend.recv_ui_prompt_state(comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// A "new" session fires session_init hooks with start_type = "new".
///
/// The hook calls `rstudioapi::navigateToFile()`, a fire-and-forget event.
/// We can't verify the frontend acts on it, but we can verify the
/// `open_editor` message arrives on the UI comm.
#[test]
fn test_session_init_hook_new() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    // Create a temp file so normalizePath() inside navigateToFile() succeeds.
    let file = tempfile::NamedTempFile::new().unwrap();
    let path = file.path().to_str().unwrap().replace('\\', "/");

    execute(
        &frontend,
        &comm_id,
        &format!(
            "setHook('positron.session_init', function(start_type) rstudioapi::navigateToFile('{path}'))"
        ),
    );

    let data = serde_json::json!({
        "method": "frontend_ready",
        "params": { "start_type": "new" },
        "id": "frontend-ready-rpc"
    });
    frontend.send_shell_comm_msg(String::from(&comm_id), data);
    frontend.recv_iopub_busy();

    // The hook sends an open_editor event to the frontend
    let event = frontend.recv_iopub_comm_msg();
    assert_eq!(event.comm_id, comm_id);
    assert_eq!(
        event.data.get("method").and_then(|v| v.as_str()),
        Some("open_editor")
    );

    let reply = frontend.recv_iopub_comm_msg();
    assert_eq!(reply.comm_id, comm_id);
    let reply = serde_json::from_value::<UiBackendReply>(reply.data).unwrap();
    assert_eq!(reply, UiBackendReply::FrontendReadyReply());
    frontend.recv_iopub_idle();
}

/// A "restart" session fires session_init hooks with start_type = "restart".
#[test]
fn test_session_init_hook_restart() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    execute(
        &frontend,
        &comm_id,
        "setHook('positron.session_init', function(start_type) cat(start_type))",
    );

    let data = serde_json::json!({
        "method": "frontend_ready",
        "params": { "start_type": "restart" },
        "id": "frontend-ready-rpc"
    });
    frontend.send_shell_comm_msg(String::from(&comm_id), data);
    frontend.recv_iopub_busy();
    frontend.assert_stream_stdout_contains("restart");
    let reply = frontend.recv_iopub_comm_msg();
    assert_eq!(reply.comm_id, comm_id);
    let reply = serde_json::from_value::<UiBackendReply>(reply.data).unwrap();
    assert_eq!(reply, UiBackendReply::FrontendReadyReply());
    frontend.recv_iopub_idle();
}

/// A "reconnect" fires session_reconnect hooks (not session_init hooks).
#[test]
fn test_session_reconnect_hook() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    execute(
        &frontend,
        &comm_id,
        "setHook('positron.session_init', function(start_type) cat('init ran'))",
    );
    execute(
        &frontend,
        &comm_id,
        "setHook('positron.session_reconnect', function() cat('reconnect ran'))",
    );

    let data = serde_json::json!({
        "method": "frontend_ready",
        "params": { "start_type": "reconnect" },
        "id": "frontend-ready-rpc"
    });
    frontend.send_shell_comm_msg(String::from(&comm_id), data);
    frontend.recv_iopub_busy();
    frontend.assert_stream_stdout_contains("reconnect ran");
    let reply = frontend.recv_iopub_comm_msg();
    assert_eq!(reply.comm_id, comm_id);
    let reply = serde_json::from_value::<UiBackendReply>(reply.data).unwrap();
    assert_eq!(reply, UiBackendReply::FrontendReadyReply());
    frontend.recv_iopub_idle();
}
