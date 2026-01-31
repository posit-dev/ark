//
// dap_breakpoints.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::io::Seek;
use std::io::Write;

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::is_idle;
use ark_test::is_start_debug;
use ark_test::is_stop_debug;
use ark_test::stream_contains;
use ark_test::stream_contains_all;
use ark_test::DummyArkFrontend;
use ark_test::SourceFile;
use tempfile::NamedTempFile;

/// Create a temp file with given content and return the file and its path.
///
/// Use this for tests that only need a file on disk without sourcing it
/// (e.g., testing hash changes or unverified breakpoints).
fn create_temp_file(code: &str) -> (NamedTempFile, String) {
    let mut file = NamedTempFile::new().unwrap();
    write!(file, "{code}").unwrap();
    let path = file.path().to_str().unwrap().replace("\\", "/");
    (file, path)
}

#[test]
fn test_dap_set_breakpoints_unverified() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let (_file, path) = create_temp_file(
        "1
2
3
",
    );

    // Set breakpoints before sourcing - they should be unverified
    let breakpoints = dap.set_breakpoints(&path, &[2]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    assert_eq!(breakpoints[0].line, Some(2));
}

#[test]
fn test_dap_clear_breakpoints() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let (_file, path) = create_temp_file(
        "x <- 1
y <- 2
z <- 3
",
    );

    // Set a breakpoint
    let breakpoints = dap.set_breakpoints(&path, &[2]);
    assert_eq!(breakpoints.len(), 1);

    // Clear all breakpoints by sending empty list
    let breakpoints = dap.set_breakpoints(&path, &[]);
    assert!(breakpoints.is_empty());
}

#[test]
fn test_dap_breakpoint_preserves_state_on_resubmit() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let (_file, path) = create_temp_file(
        "a <- 1
b <- 2
c <- 3
",
    );

    // Set initial breakpoints
    let breakpoints = dap.set_breakpoints(&path, &[2, 3]);
    assert_eq!(breakpoints.len(), 2);
    let id1 = breakpoints[0].id;
    let id2 = breakpoints[1].id;

    // Re-submit the same breakpoints - IDs should be preserved
    let breakpoints = dap.set_breakpoints(&path, &[2, 3]);
    assert_eq!(breakpoints.len(), 2);
    assert_eq!(breakpoints[0].id, id1);
    assert_eq!(breakpoints[1].id, id2);
}

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

    // Now source the file - the breakpoint is verified during parsing
    frontend.source_file(&file);

    // Breakpoint becomes verified when the function definition is parsed
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
fn test_dap_breakpoint_disabled_preserved_and_restored() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a function we can set breakpoints on
    let file = SourceFile::new(
        "
bar <- function() {
  a <- 1
  b <- 2
  c <- 3
}
",
    );

    // Set breakpoint BEFORE sourcing (on line 4: b <- 2)
    let breakpoints = dap.set_breakpoints(&file.path, &[4]);
    assert_eq!(breakpoints.len(), 1);
    let original_id = breakpoints[0].id;

    // Source the file - breakpoint becomes verified during parsing
    frontend.source_file(&file);

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, original_id);

    // Now "disable" the breakpoint by omitting it from the request
    let breakpoints = dap.set_breakpoints(&file.path, &[]);
    assert!(breakpoints.is_empty());

    // Re-enable by submitting the same line again.
    // The breakpoint should have the same ID and be immediately verified
    // (restored from disabled state without needing re-sourcing).
    let breakpoints = dap.set_breakpoints(&file.path, &[4]);
    assert_eq!(breakpoints.len(), 1);
    assert_eq!(breakpoints[0].id, original_id);
    assert!(breakpoints[0].verified);
}

#[test]
fn test_dap_breakpoint_doc_hash_change_discards_state() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create initial file
    let (mut file, path) = create_temp_file(
        "x <- 1
y <- 2
z <- 3
",
    );

    // Set breakpoints and record IDs
    let breakpoints = dap.set_breakpoints(&path, &[2, 3]);
    assert_eq!(breakpoints.len(), 2);
    let id1 = breakpoints[0].id;
    let id2 = breakpoints[1].id;

    // Modify the file content (different hash)
    file.rewind().unwrap();
    write!(file, "a <- 10\nb <- 20\nc <- 30\n").unwrap();
    file.flush().unwrap();

    // Re-submit breakpoints at the same lines
    let breakpoints = dap.set_breakpoints(&path, &[2, 3]);
    assert_eq!(breakpoints.len(), 2);

    // IDs should be new (state was discarded due to hash change)
    assert_ne!(breakpoints[0].id, id1);
    assert_ne!(breakpoints[1].id, id2);

    // Breakpoints should be unverified since they're new
    assert!(!breakpoints[0].verified);
    assert!(!breakpoints[1].verified);
}

#[test]
fn test_dap_breakpoint_line_adjustment_multiline_expr() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Breakpoints inside multiline expressions should be adjusted up to expression start
    let file = SourceFile::new(
        "
foo <- function() {
  list(
    1,
    2
  )
}
",
    );

    // Set breakpoints on lines 4 and 5 (inside the list() call)
    // Line 3 is `list(`, lines 4-5 are inside, line 6 is `)`
    let breakpoints = dap.set_breakpoints(&file.path, &[4, 5]);
    assert_eq!(breakpoints.len(), 2);
    let id1 = breakpoints[0].id;
    let id2 = breakpoints[1].id;

    // Source the file to verify breakpoints
    frontend.source_file(&file);

    // Both breakpoints should be verified and adjusted to line 3 (start of `list(`)
    let bp1 = dap.recv_breakpoint_verified();
    let bp2 = dap.recv_breakpoint_verified();

    // Check that both are our breakpoints (order may vary)
    let ids: Vec<_> = vec![bp1.id, bp2.id];
    assert!(ids.contains(&id1));
    assert!(ids.contains(&id2));

    // Both should be adjusted to line 3 (the start of the multiline expression)
    assert_eq!(bp1.line, Some(3));
    assert_eq!(bp2.line, Some(3));
}

#[test]
fn test_dap_breakpoint_line_adjustment_blank_line() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Breakpoints on whitespace/comments should be adjusted down to next statement
    let file = SourceFile::new(
        "
foo <- function() {
  # comment

  1
}
",
    );

    // Set breakpoints on line 3 (comment) and line 4 (blank line)
    // They should both be adjusted down to line 5 (the `1` statement)
    let breakpoints = dap.set_breakpoints(&file.path, &[3, 4]);
    assert_eq!(breakpoints.len(), 2);
    let id1 = breakpoints[0].id;
    let id2 = breakpoints[1].id;

    // Source the file to verify breakpoints
    frontend.source_file(&file);

    // Both breakpoints should be verified and adjusted to line 5 (the next statement)
    let bp1 = dap.recv_breakpoint_verified();
    let bp2 = dap.recv_breakpoint_verified();

    // Check that both are our breakpoints (order may vary)
    let ids: Vec<_> = vec![bp1.id, bp2.id];
    assert!(ids.contains(&id1));
    assert!(ids.contains(&id2));

    // Both should be adjusted to line 5 (the `1` statement)
    assert_eq!(bp1.line, Some(5));
    assert_eq!(bp2.line, Some(5));
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
    assert!(bp.message.is_some());
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
    assert!(bp.message.is_some());
}

#[test]
fn test_dap_breakpoints_anchor_to_same_line() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Multiple breakpoints that anchor to the same expression should all be verified together
    let file = SourceFile::new(
        "
foo <- function() {
  # comment
  list(
    1
  )
}
",
    );

    // Set breakpoints on lines 3 (comment), 4 (list(), and 5 (inside list)
    // All should anchor to line 4 (the `list(` statement)
    let breakpoints = dap.set_breakpoints(&file.path, &[3, 4, 5]);
    assert_eq!(breakpoints.len(), 3);
    let id1 = breakpoints[0].id;
    let id2 = breakpoints[1].id;
    let id3 = breakpoints[2].id;

    // Source the file to verify breakpoints
    frontend.source_file(&file);

    // All three breakpoints should be verified
    let bp1 = dap.recv_breakpoint_verified();
    let bp2 = dap.recv_breakpoint_verified();
    let bp3 = dap.recv_breakpoint_verified();

    // Check that all are our breakpoints (order may vary)
    let ids: Vec<_> = vec![bp1.id, bp2.id, bp3.id];
    assert!(ids.contains(&id1));
    assert!(ids.contains(&id2));
    assert!(ids.contains(&id3));

    // All should be adjusted to line 4 (the `list(` expression start)
    assert_eq!(bp1.line, Some(4));
    assert_eq!(bp2.line, Some(4));
    assert_eq!(bp3.line, Some(4));
}

#[test]
fn test_dap_breakpoints_isolated_per_file() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create two separate files
    let file_a = SourceFile::new(
        "
foo <- function() {
  1
}
",
    );

    let file_b = SourceFile::new(
        "
bar <- function() {
  2
}
",
    );

    // Set breakpoints in both files
    let breakpoints_a = dap.set_breakpoints(&file_a.path, &[3]);
    assert_eq!(breakpoints_a.len(), 1);
    let id_a = breakpoints_a[0].id;

    let breakpoints_b = dap.set_breakpoints(&file_b.path, &[3]);
    assert_eq!(breakpoints_b.len(), 1);
    let id_b = breakpoints_b[0].id;

    // Source only file A
    frontend.source_file(&file_a);

    // Only file A's breakpoint should be verified
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, id_a);

    // Re-query file B's breakpoints - should still be unverified
    let breakpoints_b = dap.set_breakpoints(&file_b.path, &[3]);
    assert_eq!(breakpoints_b.len(), 1);
    assert_eq!(breakpoints_b[0].id, id_b);
    assert!(!breakpoints_b[0].verified);

    // Clear file A's breakpoints
    let breakpoints_a = dap.set_breakpoints(&file_a.path, &[]);
    assert!(breakpoints_a.is_empty());

    // File B's breakpoints should still exist
    let breakpoints_b = dap.set_breakpoints(&file_b.path, &[3]);
    assert_eq!(breakpoints_b.len(), 1);
    assert_eq!(breakpoints_b[0].id, id_b);
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

/// Test that a breakpoint becomes verified immediately when execution hits it.
///
/// This tests the full breakpoint hit flow including auto-stepping.
/// When R stops inside `.ark_breakpoint()`, ark auto-steps to the actual
/// user expression, producing this message sequence:
/// - start_debug (entering .ark_breakpoint)
/// - start_debug (at actual user expression after auto-step)
/// - "Called from:" stream
/// - idle
#[test]
fn test_dap_breakpoint_verified_on_hit() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a function and a call to it
    let file = SourceFile::new(
        "
foo <- function() {
  x <- 1
  x + 1
}
foo()
",
    );

    // Set breakpoint BEFORE sourcing (on line 3: x <- 1)
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Source the file and hit the breakpoint
    frontend.source_file_and_hit_breakpoint(&file);

    // Breakpoint becomes verified when the function definition is parsed
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(3));

    // DAP side: The auto-stepping mechanism produces:
    // 1. Stopped (entering .ark_breakpoint)
    // 2. Continued (auto-step triggered)
    // 3. Continued (from stop_debug)
    // 4. Stopped (at actual user expression after auto-step)
    dap.recv_stopped();
    dap.recv_continued();
    dap.recv_continued();
    dap.recv_stopped();

    // Verify we're stopped at the right place
    let stack = dap.stack_trace();
    assert!(!stack.is_empty());
    assert_eq!(stack[0].name, "foo()");

    // Quit the debugger to clean up
    frontend.debug_send_quit();
    dap.recv_continued();

    // Receive the shell reply for the original source() request
    frontend.recv_shell_execute_reply();

    // Call foo() again to verify breakpoint is still enabled
    frontend.send_execute_request("foo()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Direct function call has a slightly different flow than source():
    // No "debug at" stream message since we're not stepping through source
    frontend.recv_iopub_async(vec![
        is_start_debug(),
        is_stop_debug(),
        stream_contains_all(&["Called from:", ".ark_breakpoint"]),
        is_idle(),
        is_start_debug(),
    ]);

    // DAP events for auto-stepping
    dap.recv_stopped();
    dap.recv_continued();
    dap.recv_continued();
    dap.recv_stopped();

    // Quit and finish
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that a breakpoint added after parsing is NOT verified when hitting another breakpoint.
///
/// This ensures that breakpoints that were never injected into the code don't get
/// incorrectly verified just because execution stopped at their location.
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

    // BP1 becomes verified when the function definition is parsed
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp1_id);

    // Now add BP2 AFTER parsing (on line 4: y <- 2)
    // This breakpoint was NOT injected into the code
    let breakpoints = dap.set_breakpoints(&file.path, &[3, 4]);
    assert_eq!(breakpoints.len(), 2);
    assert!(breakpoints[0].verified); // BP1 is still verified
    assert!(!breakpoints[1].verified); // BP2 is unverified (not injected)
    let bp2_id = breakpoints[1].id;

    // Receive the breakpoint hit messages (auto-stepping flow)
    frontend.recv_iopub_async(vec![
        stream_contains_all(&["Called from:", ".ark_breakpoint"]),
        is_start_debug(),
        is_idle(),
        is_stop_debug(),
        is_start_debug(),
        stream_contains("debug at"),
    ]);

    // DAP events for auto-stepping
    dap.recv_stopped();
    dap.recv_continued();
    dap.recv_continued();
    dap.recv_stopped();

    // We're now stopped at BP1 (line 3: x <- 1)
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "foo()");

    // Check breakpoint state - BP2 should STILL be unverified
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

    // Source the file - breakpoint becomes verified during parsing
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

    // debug(foo); foo() produces:
    // - start_debug (entering foo at first line)
    // - Stream with "debugging in:"
    // - Idle
    frontend.recv_iopub_async(vec![
        is_start_debug(),
        stream_contains("debugging in:"),
        is_idle(),
    ]);

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

    // Stepping produces: stop_debug, start_debug, Stream with "debug at", Idle
    frontend.recv_iopub_async(vec![
        is_stop_debug(),
        is_start_debug(),
        stream_contains("debug at"),
        is_idle(),
    ]);
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
