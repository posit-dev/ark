//
// dap_breakpoints_log.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;
use ark_test::SourceFile;
use dap::types::SourceBreakpoint;

/// Test that a basic log breakpoint emits the message to stderr
/// and does NOT stop execution.
#[test]
fn test_dap_log_breakpoint_emits_message() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function() {
  x <- 1
  x + 1
}
foo()
",
    );

    let breakpoints = dap.set_log_breakpoints(&file.path, &[(3, "hello from logpoint")]);
    assert_eq!(breakpoints.len(), 1);
    let bp_id = breakpoints[0].id;

    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);

    // Log breakpoint emits to stderr via the condition output path, does not stop
    frontend.assert_stream_stderr_contains("hello from logpoint");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test that `{expression}` placeholders in the log message are interpolated,
/// including references to function parameters and local variables.
#[test]
fn test_dap_log_breakpoint_interpolates_expressions() {
    let frontend = DummyArkFrontend::lock();

    if !frontend.is_installed("glue") {
        println!("Skipping test: glue package not installed");
        return;
    }

    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function(label = 'world') {
  x <- 42
  x + 1
}
foo()
",
    );

    let breakpoints = dap.set_log_breakpoints(&file.path, &[(
        4,
        "x is {x}, label is {label}, sum is {x + 1}",
    )]);
    assert_eq!(breakpoints.len(), 1);

    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    dap.recv_breakpoint_verified();

    frontend.assert_stream_stderr_contains("x is 42");
    frontend.assert_stream_stderr_contains("label is world");
    frontend.assert_stream_stderr_contains("sum is 43");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test that a log breakpoint in a loop emits once per iteration.
///
/// The breakpoint fires before the line executes, so we log `i` (the
/// loop variable, already set) rather than `x` (assigned on that line).
#[test]
fn test_dap_log_breakpoint_in_loop() {
    let frontend = DummyArkFrontend::lock();

    if !frontend.is_installed("glue") {
        println!("Skipping test: glue package not installed");
        return;
    }

    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
{
  for (i in 1:3) {
    x <- i * 10
  }
}
",
    );

    let breakpoints = dap.set_log_breakpoints(&file.path, &[(4, "iteration {i}")]);
    assert_eq!(breakpoints.len(), 1);

    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    dap.recv_breakpoint_verified();

    frontend.assert_stream_stderr_contains("iteration 1");
    frontend.assert_stream_stderr_contains("iteration 2");
    frontend.assert_stream_stderr_contains("iteration 3");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test that an error in a log message expression is reported inline
/// without stopping execution.
#[test]
fn test_dap_log_breakpoint_error_in_expression() {
    let frontend = DummyArkFrontend::lock();

    if !frontend.is_installed("glue") {
        println!("Skipping test: glue package not installed");
        return;
    }

    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function() {
  x <- 1
  x + 1
}
foo()
",
    );

    let breakpoints = dap.set_log_breakpoints(&file.path, &[(3, "value is {nonexistent_var_xyz}")]);
    assert_eq!(breakpoints.len(), 1);

    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    dap.recv_breakpoint_verified();

    // The error should appear inline in the output
    frontend.assert_stream_stderr_contains("Error:");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test that `{{}}` in the log message produces literal braces (glue escaping).
#[test]
fn test_dap_log_breakpoint_literal_braces() {
    let frontend = DummyArkFrontend::lock();

    if !frontend.is_installed("glue") {
        println!("Skipping test: glue package not installed");
        return;
    }

    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function() {
  x <- 1
  x + 1
}
foo()
",
    );

    let breakpoints = dap.set_log_breakpoints(&file.path, &[(3, "empty {{}} here")]);
    assert_eq!(breakpoints.len(), 1);

    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    dap.recv_breakpoint_verified();

    frontend.assert_stream_stderr_contains("empty {} here");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test that a log breakpoint with a condition only emits when the
/// condition is met.
#[test]
fn test_dap_log_breakpoint_with_condition() {
    let frontend = DummyArkFrontend::lock();

    if !frontend.is_installed("glue") {
        println!("Skipping test: glue package not installed");
        return;
    }

    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
{
  for (i in 1:4) {
    x <- i * 10
  }
}
",
    );

    let breakpoints = dap.set_source_breakpoints(&file.path, vec![SourceBreakpoint {
        line: 4,
        column: None,
        condition: Some("i %% 2 == 0".to_string()),
        hit_condition: None,
        log_message: Some("i={i}".to_string()),
    }]);
    assert_eq!(breakpoints.len(), 1);

    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    dap.recv_breakpoint_verified();

    let streams = frontend.recv_iopub_idle_and_flush();
    frontend.recv_shell_execute_reply();

    let stderr = streams.stderr();
    // Only even iterations satisfy the condition
    assert!(stderr.contains("i=2"));
    assert!(stderr.contains("i=4"));
    // Odd iterations should not have been logged
    assert!(!stderr.contains("i=1"));
    assert!(!stderr.contains("i=3"));
}
