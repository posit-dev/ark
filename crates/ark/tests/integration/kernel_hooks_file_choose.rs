//
// kernel_hooks_file_choose.rs
//
// Copyright (C) 2026 by Posit Software, PBC
//
//

//! `file.choose()` uses the `ptr_R_ChooseFile` callback, which only exists on
//! Unix. On Windows R shows its own dialog.

use amalthea::comm::base_comm::JsonRpcReply;
use amalthea::comm::base_comm::JsonRpcResult;
use amalthea::comm::ui_comm::SelectFileParams;
use amalthea::comm::ui_comm::UiFrontendRequest;
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::jupyter_message::Message;
use ark_test::DummyArkFrontend;
use assert_matches::assert_matches;
use serde_json::Value;

/// Executes `code`, which calls `file.choose()`, asserts the `select_file`
/// request, and replies with `result`.
fn send_file_choose(frontend: &DummyArkFrontend, code: &str, existing: bool, result: Value) {
    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let request = assert_matches!(frontend.recv_stdin(), Message::CommRequest(msg) => msg.content);
    assert_eq!(
        request,
        UiFrontendRequest::SelectFile(SelectFileParams {
            title: String::from("Select File"),
            existing,
        })
    );

    frontend.send_stdin(JsonRpcReply::Result(JsonRpcResult { result }));
}

#[test]
fn test_file_choose_returns_selected_path() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    send_file_choose(
        &frontend,
        "file.choose()",
        true,
        Value::from("/data/file.csv"),
    );

    assert_eq!(
        frontend.recv_iopub_execute_result(),
        "[1] \"/data/file.csv\""
    );
    frontend.recv_ui_prompt_state(&comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_file_choose_new_allows_new_file() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    send_file_choose(
        &frontend,
        "file.choose(new = TRUE)",
        false,
        Value::from("/data/new.csv"),
    );

    assert_eq!(
        frontend.recv_iopub_execute_result(),
        "[1] \"/data/new.csv\""
    );
    frontend.recv_ui_prompt_state(&comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_file_choose_errors_when_cancelled() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    send_file_choose(&frontend, "file.choose()", true, Value::Null);

    assert!(frontend
        .recv_iopub_execute_error()
        .contains("file choice cancelled"));
    frontend.recv_ui_prompt_state(&comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();
}

/// Without a UI comm, `file.choose()` keeps prompting in the console.
#[test]
fn test_file_choose_without_ui_comm_prompts() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };
    frontend.send_execute_request("file.choose()", options);
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    frontend.recv_stdin_input_request();
    frontend.send_stdin_input_reply(String::from("/data/file.csv"));

    assert_eq!(
        frontend.recv_iopub_execute_result(),
        "[1] \"/data/file.csv\""
    );
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}
