//
// dap_breakpoints_verification.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;
use ark_test::SourceFile;

#[test]
fn test_dap_breakpoint_verified_on_source() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a function that we can set breakpoints on.
    // The browser() at the end triggers debug mode entry.
    let file = SourceFile::new(
        "
foo <- function() {
  x <- 1
  y <- 2
  x + y
}
",
    );

    // Set breakpoint BEFORE sourcing (on line 4: y <- 2)
    let breakpoints = dap.set_breakpoints(&file.path, &[4]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Now source the file - breakpoint verified when verify code runs during evaluation
    frontend.source_file(&file);

    // Breakpoint becomes verified when the function definition is evaluated
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(4));
}

#[test]
fn test_dap_breakpoint_verified_on_execute() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a function definition.
    // Using execute_file simulates running code from an editor with location info.
    let file = SourceFile::new(
        "
bar <- function() {
  a <- 1
  b <- 2
  a + b
}
",
    );

    // Set breakpoint BEFORE executing (on line 4: b <- 2)
    let breakpoints = dap.set_breakpoints(&file.path, &[4]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Execute the file with location info - breakpoint is verified during execution
    frontend.execute_file(&file);

    // Breakpoint becomes verified when the function definition is executed
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(4));
}

#[test]
fn test_dap_breakpoint_invalid_closing_brace() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Breakpoints on closing `}` should be marked invalid with a reason message
    let file = SourceFile::new(
        "
foo <- function() {
  1
}
",
    );

    // Set breakpoint on line 4 (the closing brace `}`)
    let breakpoints = dap.set_breakpoints(&file.path, &[4]);
    assert_eq!(breakpoints.len(), 1);
    let id = breakpoints[0].id;

    // Source the file
    frontend.source_file(&file);

    // The breakpoint should be marked invalid (unverified with a message)
    let bp = dap.recv_breakpoint_invalid();
    assert_eq!(bp.id, id);
    assert!(!bp.verified);
    assert_eq!(
        bp.message,
        Some(String::from("Can't break on closing `}` brace"))
    );
}

#[test]
fn test_dap_breakpoint_invalid_empty_braces() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Breakpoints inside empty braces should be marked invalid
    let file = SourceFile::new(
        "
foo <- function() {
  # comment only, no actual code
}
",
    );

    // Set breakpoint on line 3 (inside empty braces, only a comment)
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    let id = breakpoints[0].id;

    // Source the file
    frontend.source_file(&file);

    // The breakpoint should be marked invalid (unverified with a message)
    let bp = dap.recv_breakpoint_invalid();
    assert_eq!(bp.id, id);
    assert!(!bp.verified);
    assert_eq!(
        bp.message,
        Some(String::from("Can't break inside empty braces"))
    );
}

#[test]
fn test_dap_breakpoint_trailing_expression_verified() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Breakpoint on last expression in braces should be verified (bubble-up mechanism)
    let file = SourceFile::new(
        "
foo <- function() {
  1
  2
}
",
    );

    // Set breakpoint on line 4 (the `2`, which is the last/trailing expression)
    let breakpoints = dap.set_breakpoints(&file.path, &[4]);
    assert_eq!(breakpoints.len(), 1);
    let id = breakpoints[0].id;

    // Source the file
    frontend.source_file(&file);

    // The breakpoint should become verified even though it's the trailing expression
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, id);
    assert_eq!(bp.line, Some(4));
}

#[test]
fn test_dap_breakpoint_remove_resource_readd_unverified() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Set → source → remove → source again → re-add: breakpoint should be unverified
    let file = SourceFile::new(
        "
foo <- function() {
  1
}
",
    );

    // Set breakpoint and source (verified)
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    let original_id = breakpoints[0].id;

    frontend.source_file(&file);
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, original_id);

    // Remove the breakpoint
    let breakpoints = dap.set_breakpoints(&file.path, &[]);
    assert!(breakpoints.is_empty());

    // Source again (no breakpoints to inject this time)
    frontend.source_file(&file);

    // Re-add the breakpoint at the same line
    // It should be unverified because we removed it before the second source,
    // so the disabled state was cleared when we sourced without it
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);

    // The ID should be different (new breakpoint, not restored)
    assert_ne!(breakpoints[0].id, original_id);

    // The breakpoint should be unverified (needs re-sourcing)
    assert!(!breakpoints[0].verified);

    // Source again to verify
    frontend.source_file(&file);
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, breakpoints[0].id);
}

#[test]
fn test_dap_breakpoint_partial_verification_on_error() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // When sourcing fails mid-file, breakpoints before the error should be verified,
    // breakpoints after should remain unverified
    let file = SourceFile::new(
        "
foo <- function() {
  1
}
stop('error')
bar <- function() {
  2
}
",
    );

    // Set breakpoints in both functions
    // Line 3: inside foo (before error)
    // Line 7: inside bar (after error)
    let breakpoints = dap.set_breakpoints(&file.path, &[3, 7]);
    assert_eq!(breakpoints.len(), 2);
    let id_before_error = breakpoints[0].id;
    let id_after_error = breakpoints[1].id;

    // Source the file - it will error on line 5
    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        amalthea::fixtures::dummy_frontend::ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // The breakpoint before the error should be verified
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, id_before_error);

    // Receive the error output and completion
    frontend.recv_iopub_execute_error();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();

    // Re-query breakpoints to check state
    let breakpoints = dap.set_breakpoints(&file.path, &[3, 7]);
    assert_eq!(breakpoints.len(), 2);

    // First breakpoint should still be verified (preserved state)
    assert_eq!(breakpoints[0].id, id_before_error);
    assert!(breakpoints[0].verified);

    // Second breakpoint should remain unverified (code after error wasn't reached)
    assert_eq!(breakpoints[1].id, id_after_error);
    assert!(!breakpoints[1].verified);
}

#[test]
fn test_dap_breakpoint_added_after_parse_not_verified() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a function and a call to it
    let file = SourceFile::new(
        "
foo <- function() {
  x <- 1
  y <- 2
  x + y
}
foo()
",
    );

    // Set BP1 BEFORE sourcing (on line 3: x <- 1)
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    let bp1_id = breakpoints[0].id;

    // Source the file - BP1 becomes verified during parsing
    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // BP1 becomes verified when the function definition is evaluated
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp1_id);

    // Receive the breakpoint hit messages (auto-stepping flow).
    // This must come before set_breakpoints because R may have already
    // hit the breakpoint and queued a Stopped event.
    frontend.recv_iopub_breakpoint_hit();

    dap.recv_stopped();

    // We're now stopped at BP1 (line 3: x <- 1)
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "foo()");

    // Now add BP2 (on line 4: y <- 2) while stopped.
    // BP2 was NOT injected into the code during parsing, so it should be unverified.
    let breakpoints = dap.set_breakpoints(&file.path, &[3, 4]);
    assert_eq!(breakpoints.len(), 2);
    assert_eq!(breakpoints[0].id, bp1_id);
    assert!(breakpoints[0].verified); // BP1 is verified
    let bp2_id = breakpoints[1].id;
    assert!(!breakpoints[1].verified); // BP2 is unverified (not injected)

    // Re-submit the same breakpoints - BP2 should STILL be unverified
    // because it was never injected into the code
    let breakpoints = dap.set_breakpoints(&file.path, &[3, 4]);
    assert_eq!(breakpoints.len(), 2);
    assert_eq!(breakpoints[0].id, bp1_id);
    assert!(breakpoints[0].verified); // BP1 is verified
    assert_eq!(breakpoints[1].id, bp2_id);
    assert!(!breakpoints[1].verified); // BP2 is STILL unverified

    // Quit the debugger
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that a disabled breakpoint does NOT get re-verified when stepping to its location.
///
/// When a breakpoint is disabled (removed from the active set), stepping to that line
/// via `debug()` should NOT trigger a Breakpoint event to re-verify it.
///
/// The verification here is implicit: we step to the disabled breakpoint line and
/// only expect the normal stepping DAP events (Continued/Stopped). If a Breakpoint
/// event were incorrectly sent, the test framework's cleanup would detect unexpected
/// messages.
#[test]
fn test_dap_breakpoint_disabled_inert_on_debug_stop() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a function we can set breakpoints on
    let file = SourceFile::new(
        "
foo <- function() {
  x <- 1
  y <- 2
  x + y
}
",
    );

    // Set breakpoint BEFORE sourcing (on line 4: y <- 2)
    let breakpoints = dap.set_breakpoints(&file.path, &[4]);
    assert_eq!(breakpoints.len(), 1);
    let bp_id = breakpoints[0].id;

    // Source the file - breakpoint becomes verified when the code is evaluated
    frontend.source_file(&file);
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);

    // Disable the breakpoint by clearing all breakpoints for this file.
    // Internally, verified breakpoints become "Disabled" and are preserved.
    let breakpoints = dap.set_breakpoints(&file.path, &[]);
    assert!(breakpoints.is_empty());

    // Now enter debug mode via debug(foo); foo()
    // This will stop at the first line of foo (line 3: x <- 1)
    // Note: Shell reply is delayed until debug mode exits.
    frontend.send_execute_request("debug(foo); foo()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    frontend.recv_iopub_start_debug();
    frontend.assert_stream_stdout_contains("debugging in:");
    frontend.recv_iopub_idle();

    // DAP: Stopped at first line of foo
    dap.recv_stopped();

    // Verify we're at line 3 (x <- 1)
    let stack = dap.stack_trace();
    assert!(!stack.is_empty());
    assert_eq!(stack[0].name, "foo()");

    // Step to the next line (line 4: y <- 2) - where the disabled breakpoint was.
    // If the disabled breakpoint were incorrectly re-verified, we'd receive an
    // unexpected Breakpoint event here.
    frontend.send_execute_request("n", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();
    frontend.assert_stream_stdout_contains("debug at");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // DAP: Only Continued then Stopped - no Breakpoint event
    dap.recv_continued();
    dap.recv_stopped();

    // Verify we're now at line 4 (y <- 2)
    let stack = dap.stack_trace();
    assert!(!stack.is_empty());

    // Quit the debugger
    frontend.debug_send_quit();
    dap.recv_continued();

    // Shell reply for the original debug(foo); foo() command
    frontend.recv_shell_execute_reply();
}
