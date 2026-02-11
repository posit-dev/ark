//
// dap_evaluate.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use ark_test::DummyArkFrontend;

#[test]
fn test_dap_evaluate_variable() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
local({
  x <- 42
  y <- 'hello'
  browser()
})
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let frame_id = stack[0].id;

    // Evaluate a numeric variable
    let result = dap.evaluate("x", Some(frame_id));
    assert_eq!(result, "42");

    // Evaluate a string variable
    let result = dap.evaluate("y", Some(frame_id));
    assert_eq!(result, "\"hello\"");

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_evaluate_expression() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
local({
  x <- 10
  y <- 5
  browser()
})
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let frame_id = stack[0].id;

    // Evaluate an arithmetic expression
    let result = dap.evaluate("x + y", Some(frame_id));
    assert_eq!(result, "15");

    // Evaluate a comparison
    let result = dap.evaluate("x > y", Some(frame_id));
    assert_eq!(result, "TRUE");

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_evaluate_in_different_frames() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
outer <- function() {
  outer_var <- 'from_outer'
  inner()
}
inner <- function() {
  inner_var <- 'from_inner'
  browser()
}
outer()
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert!(stack.len() >= 2, "Expected at least 2 frames");

    let inner_frame_id = stack[0].id;
    let outer_frame_id = stack[1].id;

    // Evaluate in inner frame
    let result = dap.evaluate("inner_var", Some(inner_frame_id));
    assert_eq!(result, "\"from_inner\"");

    // Evaluate in outer frame
    let result = dap.evaluate("outer_var", Some(outer_frame_id));
    assert_eq!(result, "\"from_outer\"");

    // outer_var should not be visible in inner frame
    let err = dap.evaluate_error("outer_var", Some(inner_frame_id));
    assert!(
        err.contains("not found"),
        "Expected 'not found' error, got: {err}"
    );

    // inner_var should not be visible in outer frame
    let err = dap.evaluate_error("inner_var", Some(outer_frame_id));
    assert!(
        err.contains("not found"),
        "Expected 'not found' error, got: {err}"
    );

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_evaluate_print() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
local({
  x <- c(1, 2, 3)
  df <- data.frame(a = 1:2, b = c('x', 'y'))
  browser()
})
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let frame_id = stack[0].id;

    // Using "/print " prefix returns printed output
    let result = dap.evaluate("/print x", Some(frame_id));
    assert!(
        result.contains("[1] 1 2 3"),
        "Expected printed vector output, got: {result}"
    );

    // Print a data frame
    let result = dap.evaluate("/print df", Some(frame_id));
    assert!(
        result.contains("a") && result.contains("b"),
        "Expected data frame output, got: {result}"
    );

    // Print an expression
    let result = dap.evaluate("/print sum(x)", Some(frame_id));
    assert!(
        result.contains("[1] 6"),
        "Expected sum output, got: {result}"
    );

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_evaluate_error() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
local({
  x <- 42
  browser()
})
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let frame_id = stack[0].id;

    // Evaluate an unbound variable
    let err = dap.evaluate_error("nonexistent_variable", Some(frame_id));
    assert!(
        err.contains("not found"),
        "Expected 'not found' error, got: {err}"
    );

    // Evaluate incomplete code
    let err = dap.evaluate_error("1 +", Some(frame_id));
    assert!(
        err.contains("Incomplete"),
        "Expected incomplete code error, got: {err}"
    );

    // Cause an R error during evaluation
    let err = dap.evaluate_error("stop('intentional error')", Some(frame_id));
    assert!(
        err.contains("intentional error"),
        "Expected error message, got: {err}"
    );

    // Debug session should still be alive and stopped after all these errors
    let result = dap.evaluate("x", Some(frame_id));
    assert_eq!(result, "42");

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_evaluate_top_level_browser() {
    // Top-level browser() frames have no function environment,
    // so they should fall back to global env
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Set a global variable
    frontend.send_execute_request("global_var <- 'from_global'", Default::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.debug_send_browser();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let frame_id = stack[0].id;

    // Evaluate with frame_id should work and use global env
    let result = dap.evaluate("global_var", Some(frame_id));
    assert_eq!(result, "\"from_global\"");

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_evaluate_no_frame_id() {
    // Without frame ID, evaluation occurs in the global env per the DAP protocol
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Set a global variable and enter browser
    frontend.send_execute_request("global_var <- 123", Default::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.debug_send_browser();
    dap.recv_stopped();

    // Evaluate without frame_id should use global environment
    let result = dap.evaluate("global_var", None);
    assert_eq!(result, "123");

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_evaluate_unknown_frame_id() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
local({
  browser()
})
",
    );
    dap.recv_stopped();

    // Use a bogus frame_id that doesn't exist
    let err = dap.evaluate_error("1 + 1", Some(999999));
    assert!(
        err.contains("Unknown") && err.contains("frame_id"),
        "Expected 'Unknown frame_id' error, got: {err}"
    );

    // Debug session should still be alive
    let stack = dap.stack_trace();
    assert!(!stack.is_empty(), "Stack should still be available");

    frontend.debug_send_quit();
    dap.recv_continued();
}
