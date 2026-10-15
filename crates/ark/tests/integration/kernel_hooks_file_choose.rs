//
// kernel_hooks_file_choose.rs
//
// Copyright (C) 2026 by Posit Software, PBC
//
//

//! `file.choose()` uses the `ptr_R_ChooseFile` callback, which only exists on
//! Unix. On Windows R shows its own dialog.

use amalthea::comm::base_comm::JsonRpcError;
use amalthea::comm::base_comm::JsonRpcErrorCode;
use amalthea::comm::base_comm::JsonRpcErrorData;
use amalthea::comm::base_comm::JsonRpcReply;
use amalthea::comm::base_comm::JsonRpcResult;
use amalthea::comm::ui_comm::SelectFileParams;
use amalthea::comm::ui_comm::UiFrontendRequest;
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::jupyter_message::Message;
use ark_test::DummyArkFrontend;
use ark_test::IopubExpectation;
use assert_matches::assert_matches;
use serde_json::Value;

/// Executes `code`, which calls `file.choose()`, asserts the `select_file`
/// request, and replies with `reply`.
fn send_file_choose(frontend: &DummyArkFrontend, code: &str, existing: bool, reply: JsonRpcReply) {
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

    frontend.send_stdin(reply);
}

fn result(result: Value) -> JsonRpcReply {
    JsonRpcReply::Result(JsonRpcResult { result })
}

#[test]
fn test_file_choose_returns_selected_path() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    send_file_choose(
        &frontend,
        "file.choose()",
        true,
        result(Value::from("/data/file.csv")),
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
        result(Value::from("/data/new.csv")),
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

    send_file_choose(&frontend, "file.choose()", true, result(Value::Null));

    assert!(frontend
        .recv_iopub_execute_error()
        .contains("file choice cancelled"));
    frontend.recv_ui_prompt_state(&comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();
}

#[test]
fn test_file_choose_errors_when_path_too_long() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    let path = format!("/{}", "a".repeat(2000));
    send_file_choose(&frontend, "file.choose()", true, result(Value::from(path)));

    assert!(frontend
        .recv_iopub_execute_error()
        .contains("file name too long"));
    frontend.recv_ui_prompt_state(&comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();
}

/// Interrupting while the dialog is open interrupts `file.choose()` rather
/// than cancelling it.
#[test]
fn test_file_choose_interrupt() {
    let frontend = DummyArkFrontend::lock();
    frontend.open_ui_comm();

    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };
    frontend.send_execute_request("file.choose()", options);
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    assert_matches!(frontend.recv_stdin(), Message::CommRequest(_));

    frontend.send_interrupt_request();
    frontend.recv_control_interrupt_reply();

    // R prints a newline on interrupt
    frontend.assert_stream_stderr_contains("\n");

    // Interrupt busy/idle from the control thread race with the prompt state
    // update and idle from the R thread. An unexpected execute error (such as
    // "file choice cancelled") fails here.
    frontend.recv_iopub_interleaved(&[
        &[IopubExpectation::BusyControl, IopubExpectation::IdleControl],
        &[IopubExpectation::CommMsg, IopubExpectation::IdleShell],
    ]);
    frontend.recv_shell_execute_reply();
}

/// If the frontend can't show the dialog, `file.choose()` prompts in the
/// console instead.
#[test]
fn test_file_choose_frontend_error_prompts() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    let error = JsonRpcReply::Error(JsonRpcError {
        error: JsonRpcErrorData {
            message: String::from("Dialog unavailable"),
            code: JsonRpcErrorCode::InternalError,
        },
    });
    send_file_choose(&frontend, "file.choose()", true, error);

    frontend.recv_stdin_input_request();
    frontend.send_stdin_input_reply(String::from("/data/file.csv"));

    assert_eq!(
        frontend.recv_iopub_execute_result(),
        "[1] \"/data/file.csv\""
    );
    frontend.recv_ui_prompt_state(&comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
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
