//
// dap.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::assert_file_frame;
use ark_test::assert_vdoc_frame;
use ark_test::DummyArkFrontend;
use dap::types::Thread;

#[test]
fn test_dap_initialize_and_disconnect() {
    let frontend = DummyArkFrontend::lock();

    // `start_dap()` connects and initializes, `Drop` disconnects
    let mut dap = frontend.start_dap();

    // First thing sent by frontend after connection
    assert!(matches!(
        dap.threads().as_slice(),
        [Thread { id: -1, name }] if name == "R console"
    ));
}

#[test]
fn test_dap_stopped_at_browser() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();

    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 1);

    // line: 1, column: 10 corrsponds to `browser()`
    assert_vdoc_frame(&stack[0], "<global>", 1, 10);

    // Execute an expression that doesn't advance the debugger
    // FIXME: `preserve_focus_hint` should be false
    // https://github.com/posit-dev/positron/issues/11604
    frontend.debug_send_expr("1");
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_nested_stack_frames() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = frontend.send_source(
        "
a <- function() { b() }
b <- function() { c() }
c <- function() { browser() }
a()
",
    );
    dap.recv_stopped();

    // Check stack at browser() in c - should have 3 frames: c, b, a
    let stack = dap.stack_trace();
    assert!(
        stack.len() >= 3,
        "Expected at least 3 frames, got {}",
        stack.len()
    );

    // Verify frame names (innermost to outermost)
    assert_file_frame(&stack[0], &file.filename, 4, 28);
    assert_eq!(stack[0].name, "c()");

    assert_file_frame(&stack[1], &file.filename, 3, 22);
    assert_eq!(stack[1].name, "b()");

    assert_file_frame(&stack[2], &file.filename, 2, 22);
    assert_eq!(stack[2].name, "a()");

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_recursive_function() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Recursive function that hits browser() at the base case
    let _file = frontend.send_source(
        "
factorial <- function(n) {
  if (n <= 1) {
    browser()
    return(1)
  }
  n * factorial(n - 1)
}
factorial(3)
",
    );
    dap.recv_stopped();

    // Should be at browser() when n=1, with multiple factorial() frames
    let stack = dap.stack_trace();

    // Count how many factorial() frames we have
    let factorial_frames: Vec<_> = stack.iter().filter(|f| f.name == "factorial()").collect();
    assert!(
        factorial_frames.len() >= 3,
        "Should have at least 3 factorial() frames for factorial(3), got {}",
        factorial_frames.len()
    );

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_error_during_debug() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Code that will error after browser()
    let file = frontend.send_source(
        "
{
  browser()
  stop('intentional error')
}
",
    );
    dap.recv_stopped();

    // We're at browser(), stack should have 1 frame
    let stack = dap.stack_trace();
    assert!(stack.len() >= 1, "Should have at least 1 frame");

    // Step to execute the error
    frontend.debug_send_step_command("n", &file);
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_error_in_eval() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Enter debug mode via browser() in virtual doc context
    frontend.debug_send_browser();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 1, "Should have 1 frame");

    // Evaluate an expression that causes an error.
    // Unlike stepping to an error (which exits debug), evaluating an error
    // from the console should keep us in debug mode.
    frontend.debug_send_error_expr("stop('eval error')", "eval error");
    dap.recv_continued();
    dap.recv_stopped();

    // We should still be in debug mode with the same stack
    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 1, "Should still have 1 frame after eval error");

    // Clean exit
    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_nested_browser() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Enter debug mode via browser() in virtual doc context
    frontend.debug_send_browser();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 1, "Should have 1 frame at Browse[1]>");

    // Enter nested debug by calling a function with debugonce
    frontend.send_execute_request(
        "debugonce(identity); identity(1)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    frontend.recv_iopub_stop_debug();
    frontend.assert_stream_stdout_contains("debugging in:");
    frontend.recv_iopub_start_debug();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // DAP: Continued (exiting Browse[1]>), then Stopped (entering Browse[2]>)
    dap.recv_continued();
    dap.recv_stopped();

    // Stack now shows 2 frames: identity() and the original browser frame
    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 2, "Should have 2 frames at Browse[2]>");
    assert_eq!(stack[0].name, "identity()");

    // Step with `n` to return to parent browser (Browse[1]>)
    frontend.send_execute_request("n", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();
    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // DAP: Continued (left identity) then Stopped (back at parent browser)
    dap.recv_continued();
    dap.recv_stopped();

    // Back to 1 frame at Browse[1]>
    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 1, "Should have 1 frame back at Browse[1]>");

    // Now quit entirely
    frontend.debug_send_quit();
    dap.recv_continued();
}
