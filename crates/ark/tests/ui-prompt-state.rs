//
// ui-prompt-state.rs
//
// Copyright (C) 2026 by Posit Software, PBC
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

/// After a normal execution, the kernel sends a `prompt_state` event
/// reflecting the default R prompt.
#[test]
fn test_prompt_state_after_execution() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    frontend.send_execute_request("1 + 1", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_ui_busy(&comm_id, true);
    frontend.recv_ui_busy(&comm_id, false);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 2");
    let prompt = frontend.recv_ui_prompt_state(&comm_id);
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
    frontend.recv_ui_busy(&comm_id, true);
    frontend.recv_ui_busy(&comm_id, false);
    let prompt = frontend.recv_ui_prompt_state(&comm_id);
    assert_eq!(prompt.input_prompt, "hello> ");
    assert_eq!(prompt.continuation_prompt, "+ ");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.send_execute_request("options(prompt = '> ')", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_ui_busy(&comm_id, true);
    frontend.recv_ui_busy(&comm_id, false);
    let prompt = frontend.recv_ui_prompt_state(&comm_id);
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
    frontend.recv_ui_busy(&comm_id, false);
    frontend.recv_ui_busy(&comm_id, true);
    frontend.recv_ui_busy(&comm_id, false);
    let prompt = frontend.recv_ui_prompt_state(&comm_id);
    assert_eq!(prompt.input_prompt, "Browse[1]> ");
    assert_eq!(prompt.continuation_prompt, "+ ");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Exit the browser with `Q` — prompt should return to normal.
    // `Q` in the browser only produces a single busy=false.
    frontend.send_execute_request("Q", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_ui_busy(&comm_id, false);
    let prompt = frontend.recv_ui_prompt_state(&comm_id);
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
    frontend.recv_ui_busy(&comm_id, true);
    frontend.recv_ui_busy(&comm_id, false);
    let prompt = frontend.recv_ui_prompt_state(&comm_id);
    assert_eq!(prompt.input_prompt, "> ");
    assert_eq!(prompt.continuation_prompt, "... ");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.send_execute_request("options(continue = '+ ')", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_ui_busy(&comm_id, true);
    frontend.recv_ui_busy(&comm_id, false);
    let prompt = frontend.recv_ui_prompt_state(&comm_id);
    assert_eq!(prompt.continuation_prompt, "+ ");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}
