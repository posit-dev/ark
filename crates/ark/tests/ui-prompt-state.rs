//
// ui-prompt-state.rs
//
// Copyright (C) 2026 by Posit Software, PBC
//
//

use amalthea::comm::ui_comm::PromptStateParams;
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

/// Receive a UI comm event from IOPub and assert it is a `busy` event.
#[track_caller]
fn recv_ui_busy(frontend: &DummyArkFrontend, comm_id: &str, expected: bool) {
    let msg = frontend.recv_iopub_comm_msg();
    assert_eq!(msg.comm_id, comm_id);
    assert_eq!(
        msg.data.get("method").and_then(|v| v.as_str()),
        Some("busy"),
        "Expected busy event, got: {:?}",
        msg.data
    );
    assert_eq!(msg.data["params"]["busy"], expected);
}

/// Receive a UI comm event from IOPub and assert it is a `prompt_state` event.
/// Returns the parsed parameters for further assertions.
#[track_caller]
fn recv_ui_prompt_state(frontend: &DummyArkFrontend, comm_id: &str) -> PromptStateParams {
    let msg = frontend.recv_iopub_comm_msg();
    assert_eq!(msg.comm_id, comm_id);
    assert_eq!(
        msg.data.get("method").and_then(|v| v.as_str()),
        Some("prompt_state"),
        "Expected prompt_state event, got: {:?}",
        msg.data
    );
    serde_json::from_value(msg.data["params"].clone()).expect("Failed to parse PromptStateParams")
}

/// After a normal execution, the kernel sends a `prompt_state` event
/// reflecting the default R prompt.
#[test]
fn test_prompt_state_after_execution() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    frontend.send_execute_request("1 + 1", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    recv_ui_busy(&frontend, &comm_id, true);
    recv_ui_busy(&frontend, &comm_id, false);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 2");
    let prompt = recv_ui_prompt_state(&frontend, &comm_id);
    assert_eq!(prompt.input_prompt, "> ");
    assert_eq!(prompt.continuation_prompt, "+ ");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// After changing the prompt via `options()`, the next execution's
/// `prompt_state` event reflects the new prompt.
#[test]
fn test_prompt_state_custom_prompt() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    // Change the prompt
    frontend.send_execute_request(
        "options(prompt = 'hello> ')",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    recv_ui_busy(&frontend, &comm_id, true);
    recv_ui_busy(&frontend, &comm_id, false);
    let prompt = recv_ui_prompt_state(&frontend, &comm_id);
    assert_eq!(prompt.input_prompt, "hello> ");
    assert_eq!(prompt.continuation_prompt, "+ ");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.send_execute_request("options(prompt = '> ')", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    recv_ui_busy(&frontend, &comm_id, true);
    recv_ui_busy(&frontend, &comm_id, false);
    let prompt = recv_ui_prompt_state(&frontend, &comm_id);
    assert_eq!(prompt.input_prompt, "> ");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// When entering the debugger via `browser()`, the `prompt_state` event
/// reports the browser prompt (e.g. `Browse[1]> `).
#[test]
fn test_prompt_state_browser() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    // Enter the browser. The busy sequence differs from normal execution:
    // R briefly goes idle entering the browser's ReadConsole, then busy
    // again for handle_active_request, then idle.
    frontend.send_execute_request("browser()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    recv_ui_busy(&frontend, &comm_id, false);
    recv_ui_busy(&frontend, &comm_id, true);
    recv_ui_busy(&frontend, &comm_id, false);
    let prompt = recv_ui_prompt_state(&frontend, &comm_id);
    assert_eq!(prompt.input_prompt, "Browse[1]> ");
    assert_eq!(prompt.continuation_prompt, "+ ");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Exit the browser with `Q` — prompt should return to normal.
    // `Q` in the browser only produces a single busy=false.
    frontend.send_execute_request("Q", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    recv_ui_busy(&frontend, &comm_id, false);
    let prompt = recv_ui_prompt_state(&frontend, &comm_id);
    assert_eq!(prompt.input_prompt, "> ");
    assert_eq!(prompt.continuation_prompt, "+ ");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Changing the continuation prompt via `options(continue = ...)` is
/// reflected in the next `prompt_state` event.
#[test]
fn test_prompt_state_custom_continuation() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    frontend.send_execute_request(
        "options(continue = '... ')",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    recv_ui_busy(&frontend, &comm_id, true);
    recv_ui_busy(&frontend, &comm_id, false);
    let prompt = recv_ui_prompt_state(&frontend, &comm_id);
    assert_eq!(prompt.input_prompt, "> ");
    assert_eq!(prompt.continuation_prompt, "... ");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.send_execute_request("options(continue = '+ ')", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    recv_ui_busy(&frontend, &comm_id, true);
    recv_ui_busy(&frontend, &comm_id, false);
    let prompt = recv_ui_prompt_state(&frontend, &comm_id);
    assert_eq!(prompt.continuation_prompt, "+ ");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}
