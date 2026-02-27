//
// dap_breakpoints_conditional.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;
use ark_test::SourceFile;
use dap::types::SourceBreakpoint;

/// Test that a conditional breakpoint stops when the condition is true.
#[test]
fn test_dap_conditional_breakpoint_stops_when_true() {
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

    // Set conditional breakpoint on line 3 (x <- 1) with always-true condition
    let breakpoints = dap.set_conditional_breakpoints(&file.path, &[(3, "TRUE")]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Source the file and hit the breakpoint
    frontend.source_file_and_hit_breakpoint(&file);

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(3));

    dap.recv_stopped();
    dap.assert_top_frame("foo()");
    dap.assert_top_frame_line(3);
    dap.assert_top_frame_file(&file);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that a conditional breakpoint does NOT stop when the condition is false.
#[test]
fn test_dap_conditional_breakpoint_skips_when_false() {
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

    // Set conditional breakpoint with always-false condition
    let breakpoints = dap.set_conditional_breakpoints(&file.path, &[(3, "FALSE")]);
    assert_eq!(breakpoints.len(), 1);
    let bp_id = breakpoints[0].id;

    // Source the file. The breakpoint injection runs but condition is FALSE,
    // so browser() is never forced and execution completes normally.
    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // The breakpoint gets verified (the injected code runs), but no stop occurs
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);

    // No execute result: source() returns invisibly
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test conditional breakpoint in a for loop that only stops on specific iterations.
///
/// With condition `i == 2`, the breakpoint should skip i=1, stop at i=2,
/// and skip i=3 after continuing.
#[test]
fn test_dap_conditional_breakpoint_for_loop() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
{
  for (i in 1:3) {
    x <- i * 2
  }
}
",
    );

    // Set conditional breakpoint on line 4 (x <- i * 2) with condition i == 2
    let breakpoints = dap.set_conditional_breakpoints(&file.path, &[(4, "i == 2")]);
    assert_eq!(breakpoints.len(), 1);
    let bp_id = breakpoints[0].id;

    // Source the file. i=1 skips the breakpoint, i=2 hits it.
    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(4));

    // i=1: condition is FALSE, no stop. i=2: condition is TRUE, breakpoint fires.
    frontend.recv_iopub_breakpoint_hit();
    dap.recv_stopped();
    dap.assert_top_frame_line(4);
    dap.assert_top_frame_file(&file);

    // Stopped before `x <- i * 2` executes, so `x` still has its i=1 value
    let frame_id = dap.stack_trace()[0].id;
    assert_eq!(dap.evaluate("x", Some(frame_id)), "2");

    // Continue. i=3: condition is FALSE, loop ends, execution completes.
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    // No execute result: source() returns invisibly
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.recv_continued();

    frontend.recv_shell_execute_reply();
}

/// Test conditional breakpoint where the condition uses a local variable.
#[test]
fn test_dap_conditional_breakpoint_references_local_variable() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function(n) {
  x <- n * 10
  x
}
foo(5)
",
    );

    // Condition references the parameter `n`
    let breakpoints = dap.set_conditional_breakpoints(&file.path, &[(3, "n > 3")]);
    assert_eq!(breakpoints.len(), 1);

    // Source the file. foo(5) has n=5 > 3, so breakpoint should fire.
    frontend.source_file_and_hit_breakpoint(&file);

    dap.recv_breakpoint_verified();
    dap.recv_stopped();
    dap.assert_top_frame("foo()");
    dap.assert_top_frame_line(3);
    let frame_id = dap.stack_trace()[0].id;
    assert_eq!(dap.evaluate("n", Some(frame_id)), "5");

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that a conditional breakpoint with an error in the condition still stops.
///
/// Per our implementation, if the condition expression errors, we treat it as
/// TRUE to avoid silently swallowing bugs in conditions.
#[test]
fn test_dap_conditional_breakpoint_error_in_condition_stops() {
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

    // Condition references a nonexistent variable, which will error
    let breakpoints =
        dap.set_conditional_breakpoints(&file.path, &[(3, "nonexistent_variable_xyz")]);
    assert_eq!(breakpoints.len(), 1);

    // The condition errors, so the breakpoint should fire (treated as TRUE)
    frontend.source_file_and_hit_breakpoint(&file);

    dap.recv_breakpoint_verified();
    dap.recv_stopped();
    dap.assert_top_frame("foo()");
    dap.assert_top_frame_line(3);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test mixed conditional and unconditional breakpoints in a single
/// `SetBreakpoints` request. The conditional one (FALSE) should be skipped,
/// the unconditional one should stop.
#[test]
fn test_dap_mixed_conditional_and_unconditional_breakpoints() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function() {
  a <- 1
  b <- 2
  c <- 3
}
foo()
",
    );

    let breakpoints = dap.set_source_breakpoints(&file.path, vec![
        SourceBreakpoint {
            line: 3,
            column: None,
            condition: Some("FALSE".to_string()),
            hit_condition: None,
            log_message: None,
        },
        SourceBreakpoint {
            line: 4,
            column: None,
            condition: None,
            hit_condition: None,
            log_message: None,
        },
    ]);
    assert_eq!(breakpoints.len(), 2);

    frontend.source_file_and_hit_breakpoint(&file);

    // Both breakpoints get verified as the injected code runs
    dap.recv_breakpoint_verified();
    dap.recv_breakpoint_verified();

    // Line 3 condition is FALSE so it's skipped, line 4 is unconditional so it stops
    dap.recv_stopped();
    dap.assert_top_frame("foo()");
    dap.assert_top_frame_line(4);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that a condition returning a non-logical value (e.g. a number)
/// is treated as TRUE and the breakpoint fires.
#[test]
fn test_dap_conditional_breakpoint_non_logical_condition_stops() {
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

    // Condition evaluates to the string "hello", not a logical
    let breakpoints = dap.set_conditional_breakpoints(&file.path, &[(3, "'hello'")]);
    assert_eq!(breakpoints.len(), 1);

    frontend.source_file_and_hit_breakpoint(&file);

    dap.recv_breakpoint_verified();
    dap.recv_stopped();
    dap.assert_top_frame("foo()");
    dap.assert_top_frame_line(3);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that a condition returning a value not coercible to logical (e.g. an
/// environment) errors during `as.logical()` and is treated as TRUE.
#[test]
fn test_dap_conditional_breakpoint_non_coercible_condition_stops() {
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

    // `as.logical(environment())` errors: cannot coerce type 'environment' to logical
    let breakpoints = dap.set_conditional_breakpoints(&file.path, &[(3, "environment()")]);
    assert_eq!(breakpoints.len(), 1);

    frontend.source_file_and_hit_breakpoint(&file);

    dap.recv_breakpoint_verified();
    dap.recv_stopped();
    dap.assert_top_frame("foo()");
    dap.assert_top_frame_line(3);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that a numeric zero condition is coerced to FALSE (breakpoint skipped).
#[test]
fn test_dap_conditional_breakpoint_numeric_zero_skips() {
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

    // `0` coerces to FALSE via `as.logical()`
    let breakpoints = dap.set_conditional_breakpoints(&file.path, &[(3, "0")]);
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

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Test that a non-zero numeric condition is coerced to TRUE (breakpoint fires).
#[test]
fn test_dap_conditional_breakpoint_numeric_nonzero_stops() {
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

    // `42` coerces to TRUE via `as.logical()`
    let breakpoints = dap.set_conditional_breakpoints(&file.path, &[(3, "42")]);
    assert_eq!(breakpoints.len(), 1);

    frontend.source_file_and_hit_breakpoint(&file);

    dap.recv_breakpoint_verified();
    dap.recv_stopped();
    dap.assert_top_frame("foo()");
    dap.assert_top_frame_line(3);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that a numeric expression is coerced: `i - 1` is falsy when `i == 1`
/// and truthy when `i == 2`.
#[test]
fn test_dap_conditional_breakpoint_numeric_expression_in_loop() {
    let frontend = DummyArkFrontend::lock();
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

    // `i - 1` evaluates to 0 (falsy) on first iteration, non-zero (truthy) after
    let breakpoints = dap.set_conditional_breakpoints(&file.path, &[(4, "i - 1")]);
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

    // i=1: `i - 1` is 0 → FALSE, skipped. i=2: `i - 1` is 1 → TRUE, stops.
    frontend.recv_iopub_breakpoint_hit();
    dap.recv_stopped();
    dap.assert_top_frame_line(4);
    dap.assert_top_frame_file(&file);

    let frame_id = dap.stack_trace()[0].id;
    assert_eq!(dap.evaluate("i", Some(frame_id)), "2L");

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that changing a condition via SetBreakpoints takes effect without re-sourcing.
///
/// The condition is stored in the Rust DAP state and queried at runtime,
/// so updating it via a new SetBreakpoints request is picked up on the
/// next breakpoint hit.
#[test]
fn test_dap_conditional_breakpoint_condition_update() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function(n) {
  x <- n * 10
  x
}
",
    );

    // Start with condition that won't match
    let breakpoints = dap.set_conditional_breakpoints(&file.path, &[(3, "n > 100")]);
    assert_eq!(breakpoints.len(), 1);

    // Source the file (defines the function, breakpoint gets verified)
    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    dap.recv_breakpoint_verified();
    // No execute result: source() returns invisibly
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Call foo(1) - condition n > 100 is false, should not stop
    frontend.send_execute_request("foo(1)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Now update the condition to something that will match
    let breakpoints = dap.set_conditional_breakpoints(&file.path, &[(3, "n > 0")]);
    assert_eq!(breakpoints.len(), 1);
    // Breakpoint should still be verified (preserved from before)
    assert!(breakpoints[0].verified);

    // Call foo(1) again - condition n > 0 is now true, should stop
    frontend.send_execute_request("foo(1)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_breakpoint_hit();

    dap.recv_stopped();
    dap.assert_top_frame("foo()");
    dap.assert_top_frame_line(3);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}
