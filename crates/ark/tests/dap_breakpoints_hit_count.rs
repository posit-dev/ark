//
// dap_breakpoints_hit_count.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;
use ark_test::SourceFile;
use dap::types::SourceBreakpoint;

/// Hit count breakpoint with threshold 3 in a loop 1:5. The breakpoint
/// skips iterations 1 and 2, then fires on every iteration from 3 onward.
/// We continue once to confirm it fires again at i=4.
#[test]
fn test_dap_hit_count_in_loop() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function() {
  for (i in 1:5) {
    x <- i * 2
  }
}
foo()
",
    );

    let breakpoints = dap.set_hit_count_breakpoints(&file.path, &[(4, "3")]);
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
    assert_eq!(bp.line, Some(4));

    // i=1: hit=1 (<3) skip, i=2: hit=2 (<3) skip, i=3: hit=3 (>=3) STOP
    frontend.recv_iopub_breakpoint_hit();
    dap.recv_stopped();
    dap.assert_top_frame_line(4);
    dap.assert_top_frame_file(&file);

    let frame_id = dap.stack_trace()[0].id;
    assert_eq!(dap.evaluate("i", Some(frame_id)), "3L");

    // Continue. i=4: hit=4 (>=3) STOP again
    frontend.debug_send_step_command("c");
    dap.recv_continued();
    dap.recv_stopped();

    let frame_id = dap.stack_trace()[0].id;
    assert_eq!(dap.evaluate("i", Some(frame_id)), "4L");

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Hit counts reset between runs. Sourcing the same file twice should
/// apply the threshold fresh each time, stopping at the same iteration.
#[test]
fn test_dap_hit_count_resets_between_runs() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function() {
  for (i in 1:5) {
    x <- i * 2
  }
}
foo()
",
    );

    let breakpoints = dap.set_hit_count_breakpoints(&file.path, &[(4, "3")]);
    assert_eq!(breakpoints.len(), 1);
    let bp_id = breakpoints[0].id;

    // First run: stops at i=3 (3rd hit)
    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);

    frontend.recv_iopub_breakpoint_hit();
    dap.recv_stopped();

    let frame_id = dap.stack_trace()[0].id;
    assert_eq!(dap.evaluate("i", Some(frame_id)), "3L");

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();

    // Second run: if hit counts weren't reset, the counter would start
    // at 6 (from previous run) and fire immediately at i=1. Instead it
    // should start fresh and stop at i=3 again.
    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    frontend.recv_iopub_breakpoint_hit();
    dap.recv_stopped();

    let frame_id = dap.stack_trace()[0].id;
    assert_eq!(dap.evaluate("i", Some(frame_id)), "3L");

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_dap_hit_count_threshold_zero() {
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

    let breakpoints = dap.set_hit_count_breakpoints(&file.path, &[(3, "0")]);
    assert_eq!(breakpoints.len(), 1);

    frontend.source_file_and_hit_breakpoint(&file);

    let _bp = dap.recv_breakpoint_verified();
    dap.recv_stopped();
    dap.assert_top_frame_line(3);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Hit count of 1 behaves like an unconditional breakpoint (fires on every hit).
#[test]
fn test_dap_hit_count_threshold_one() {
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

    let breakpoints = dap.set_hit_count_breakpoints(&file.path, &[(3, "1")]);
    assert_eq!(breakpoints.len(), 1);

    frontend.source_file_and_hit_breakpoint(&file);

    let _bp = dap.recv_breakpoint_verified();
    dap.recv_stopped();
    dap.assert_top_frame_line(3);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Hit count threshold that exceeds the total number of iterations.
/// The breakpoint is verified but never fires.
#[test]
fn test_dap_hit_count_never_met() {
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

    let breakpoints = dap.set_hit_count_breakpoints(&file.path, &[(4, "10")]);
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

    // Execution completes without stopping
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Hit count combined with a condition. Per the DAP spec, the condition is
/// evaluated first; the hit count only increments when the condition is TRUE.
///
/// With condition "i %% 2 == 0" (even numbers) and hit_condition "2":
/// - i=1: condition FALSE -> skip (hit=0)
/// - i=2: condition TRUE -> hit=1 (<2) -> skip
/// - i=3: condition FALSE -> skip (hit=1)
/// - i=4: condition TRUE -> hit=2 (>=2) -> STOP
#[test]
fn test_dap_hit_count_with_condition() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function() {
  for (i in 1:10) {
    x <- i * 2
  }
}
foo()
",
    );

    let breakpoints = dap.set_source_breakpoints(&file.path, vec![SourceBreakpoint {
        line: 4,
        column: None,
        condition: Some("i %% 2 == 0".to_string()),
        hit_condition: Some("2".to_string()),
        log_message: None,
    }]);
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

    // Stops at i=4: the 2nd time condition is TRUE
    frontend.recv_iopub_breakpoint_hit();
    dap.recv_stopped();

    let frame_id = dap.stack_trace()[0].id;
    assert_eq!(dap.evaluate("i", Some(frame_id)), "4L");

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Verify that the hit count does NOT increment when the condition is FALSE.
///
/// If hit count were incremented before condition evaluation (the wrong
/// ordering), the hit count would reach 3 by the time i==3, and the
/// breakpoint would fire at i=3 on the first call. With correct ordering,
/// condition `i == 3` matches only once per call, so the hit count only
/// reaches 1 on the first call (<2 threshold), and the breakpoint does
/// not fire until the second call.
#[test]
fn test_dap_hit_count_not_incremented_when_condition_false() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function() {
  for (i in 1:5) {
    x <- i * 2
  }
}
foo()
foo()
",
    );

    let breakpoints = dap.set_source_breakpoints(&file.path, vec![SourceBreakpoint {
        line: 4,
        column: None,
        condition: Some("i == 3".to_string()),
        hit_condition: Some("2".to_string()),
        log_message: None,
    }]);
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

    // First foo() call: condition `i == 3` is TRUE once -> hit_count=1 (<2) -> no stop.
    // Second foo() call: condition `i == 3` is TRUE once more -> hit_count=2 (>=2) -> STOP.
    frontend.recv_iopub_breakpoint_hit();
    dap.recv_stopped();

    let frame_id = dap.stack_trace()[0].id;
    assert_eq!(dap.evaluate("i", Some(frame_id)), "3L");

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Hit count combined with a log message. The log output is only emitted
/// once the hit count threshold is reached.
#[test]
fn test_dap_hit_count_with_log_message() {
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

    // Log breakpoint with hit count 2: log on iterations 2 and 3 only
    let breakpoints = dap.set_source_breakpoints(&file.path, vec![SourceBreakpoint {
        line: 4,
        column: None,
        condition: None,
        hit_condition: Some("2".to_string()),
        log_message: Some("iteration {i}".to_string()),
    }]);
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

    // Log breakpoints never stop, so execution completes.
    // Iterations 2 and 3 produce log output (hit >= 2), but not iteration 1.
    frontend.assert_stream_stderr_contains("iteration 2");
    frontend.assert_stream_stderr_contains("iteration 3");
    let streams = frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    assert!(!streams.stderr().contains("iteration 1"));
}

/// Non-numeric hit condition emits a diagnostic and fires the breakpoint
/// (same behaviour as a condition that errors).
#[test]
fn test_dap_hit_count_invalid_value_emits_diagnostic() {
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

    let breakpoints = dap.set_hit_count_breakpoints(&file.path, &[(3, "abc")]);
    assert_eq!(breakpoints.len(), 1);

    // Inline the breakpoint-hit flow so we can assert stderr before idle
    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    frontend.recv_iopub_start_debug();
    frontend.assert_stream_stderr_contains("```breakpoint");
    frontend.assert_stream_stderr_contains("Expected a positive integer");
    frontend.assert_stream_stderr_contains("```");
    frontend.drain_streams();
    frontend.recv_iopub_idle();

    dap.recv_breakpoint_verified();
    dap.recv_stopped();
    dap.assert_top_frame_line(3);

    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}
