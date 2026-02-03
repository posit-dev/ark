//
// dap_breakpoints.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::io::Seek;
use std::io::Write;
use std::thread;
use std::time::Duration;

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::is_execute_result;
use ark_test::is_idle;
use ark_test::is_start_debug;
use ark_test::is_stop_debug;
use ark_test::stream_contains;
use ark_test::DapClient;
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

    // Source the file - breakpoint becomes verified during evaluation
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

/// Test that document hash changes cause breakpoint state to be discarded.
///
/// This is part of the document change invalidation coverage: when a file's
/// content changes (detected via hash), breakpoint IDs are regenerated and
/// state is reset. This complements the LSP `did_change_document()` unit tests
/// in `dap.rs`.
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

/// Test the full breakpoint flow: source, verify, hit, and hit again.
///
/// This tests end-to-end breakpoint functionality including auto-stepping.
/// When R stops inside `.ark_breakpoint()`, ark auto-steps to the actual
/// user expression, producing this message sequence:
/// - start_debug (entering .ark_breakpoint)
/// - start_debug (at actual user expression after auto-step)
/// - "Called from:" stream
/// - idle
#[test]
fn test_dap_breakpoint_source_and_hit() {
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

    // Breakpoint becomes verified when the function definition is evaluated
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(3));

    dap.recv_auto_step_through();
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
    frontend.recv_iopub_breakpoint_hit_direct();

    dap.recv_auto_step_through();
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

    // BP1 becomes verified when the function definition is evaluated
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp1_id);

    // Receive the breakpoint hit messages (auto-stepping flow).
    // This must come before set_breakpoints because R may have already
    // hit the breakpoint and queued a Stopped event.
    frontend.recv_iopub_breakpoint_hit();

    dap.recv_auto_step_through();
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

/// Basic DAP reconnection test.
///
/// This test, along with the other reconnection tests, helps cover the
/// "multiple sessions with different breakpoint states" scenario from the
/// test coverage plan. Disconnection/reconnection simulates switching between
/// sessions or restarting the debugger.
#[test]
fn test_dap_reconnect_basic() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Get the port before disconnecting
    let port = dap.port();

    // Basic sanity check - threads request works
    let threads = dap.threads();
    assert_eq!(threads.len(), 1);

    // Disconnect the client and drop to close TCP connection
    dap.disconnect();
    drop(dap);

    // Give the server time to process disconnect and loop back to accept()
    thread::sleep(Duration::from_millis(100));

    // Reconnect to the same DAP server
    let mut dap = DapClient::connect("127.0.0.1", port).unwrap();
    dap.initialize();
    dap.attach();

    // Basic sanity check - threads request works after reconnection
    let threads = dap.threads();
    assert_eq!(threads.len(), 1);
}

/// Test that breakpoint state is preserved across DAP reconnection.
///
/// Part of multi-session test coverage: verifies that breakpoints set in one
/// "session" persist when the debugger reconnects, as long as the file hasn't
/// changed.
#[test]
fn test_dap_breakpoint_state_preserved_on_reconnect() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function() {
  x <- 1
  y <- 2
}
",
    );

    // Set breakpoint and source to verify it
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    let bp_id = breakpoints[0].id;

    frontend.source_file(&file);
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);

    // Get the port before disconnecting
    let port = dap.port();

    // Disconnect the client and drop to close TCP connection
    dap.disconnect();
    drop(dap);

    // Give the server time to process disconnect and loop back to accept()
    thread::sleep(Duration::from_millis(100));

    // Reconnect to the same DAP server (with retry since server needs time to accept)
    let mut dap = DapClient::connect("127.0.0.1", port).unwrap();
    dap.initialize();
    dap.attach();

    // Re-query breakpoints - state should be preserved (same ID, still verified)
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert_eq!(breakpoints[0].id, bp_id);
    assert!(breakpoints[0].verified);
}

/// Test that breakpoint state is reset when file changes during disconnection.
///
/// This test covers both multi-session and document change invalidation scenarios:
/// - Simulates a "background session" modifying a file while disconnected
/// - Verifies that breakpoints are reset to unverified when the file hash changes
/// - This is the integration-level test for document change invalidation
///   (complements the unit tests in `dap.rs` for `did_change_document()`)
#[test]
fn test_dap_breakpoint_state_reset_on_reconnect_after_file_change() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let (mut file, path) = create_temp_file(
        "foo <- function() {
  x <- 1
  y <- 2
}
",
    );

    // Set breakpoint and record ID (unverified since we're using temp file)
    let breakpoints = dap.set_breakpoints(&path, &[2]);
    assert_eq!(breakpoints.len(), 1);
    let original_id = breakpoints[0].id;

    // Get the port before disconnecting
    let port = dap.port();

    // Disconnect the client and drop to close TCP connection
    dap.disconnect();
    drop(dap);

    // Give the server time to process disconnect and loop back to accept()
    thread::sleep(Duration::from_millis(100));

    // Modify the file content while disconnected (simulates background session scenario)
    file.rewind().unwrap();
    write!(file, "bar <- function() {{\n  a <- 10\n  b <- 20\n}}\n").unwrap();
    file.flush().unwrap();

    // Reconnect to the same DAP server (with retry since server needs time to accept)
    let mut dap = DapClient::connect("127.0.0.1", port).unwrap();
    dap.initialize();
    dap.attach();

    // Re-query breakpoints - state should be reset due to hash change
    let breakpoints = dap.set_breakpoints(&path, &[2]);
    assert_eq!(breakpoints.len(), 1);

    // ID should be different (state was discarded due to hash change)
    assert_ne!(breakpoints[0].id, original_id);

    // Breakpoint should be unverified
    assert!(!breakpoints[0].verified);
}

/// Regression test: stepping through a top-level `{}` block with breakpoints
/// must not cause subsequent `{}` blocks (without breakpoints) to enter the debugger.
///
/// This prevents a bug where `RDEBUG` could be accidentally set on the global
/// environment, causing unrelated code to drop into the debugger.
#[test]
fn test_dap_toplevel_braces_no_global_debug() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a braced block containing a breakpoint
    let file = SourceFile::new(
        "
{
  x <- 1
  y <- 2
}
",
    );

    // Set breakpoint on line 3 (x <- 1)
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);

    // Source the file and hit the breakpoint
    frontend.source_file_and_hit_breakpoint(&file);

    // Breakpoint becomes verified when the block is evaluated
    dap.recv_breakpoint_verified();

    dap.recv_auto_step_through();
    dap.recv_stopped();

    // Quit the debugger to exit cleanly
    frontend.debug_send_quit();
    dap.recv_continued();

    // Receive the shell reply for the original source() request
    frontend.recv_shell_execute_reply();

    // Now execute another `{}` block without any breakpoints.
    // This should complete normally without entering the debugger.
    frontend.execute_request_invisibly(
        "{
  a <- 10
  b <- 20
}",
    );

    // If we reached here without hanging or panicking, the test passes.
    // The execute_request_invisibly helper asserts the normal message flow
    // (busy -> execute_input -> idle -> execute_reply) which would fail
    // if R entered the debugger unexpectedly.
}

/// Test that `source(file, echo=TRUE)` correctly handles breakpoints.
///
/// The source() hook explicitly supports echo=TRUE (used by Positron), so this
/// tests that breakpoints work correctly with this option.
#[test]
fn test_dap_source_with_echo() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = SourceFile::new(
        "
foo <- function() {
  x <- 1
  x
}
",
    );

    // Set breakpoint BEFORE sourcing (on line 3: x <- 1)
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Source the file with echo=TRUE
    // The message flow is the same as normal source() - echo=TRUE just affects
    // what R prints during sourcing, but we don't need to capture that here.
    frontend.send_execute_request(
        &format!("source('{}', echo=TRUE)", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Breakpoint becomes verified when the function definition is evaluated
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(3));

    // Call foo() to hit the breakpoint
    frontend.send_execute_request("foo()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Direct function call - use recv_iopub_breakpoint_hit_direct which handles
    // the debug message flow
    frontend.recv_iopub_breakpoint_hit_direct();

    dap.recv_auto_step_through();
    dap.recv_stopped();

    // Verify we're stopped at the right place
    let stack = dap.stack_trace();
    assert!(!stack.is_empty());
    assert_eq!(stack[0].name, "foo()");
    assert_eq!(stack[0].line, 3);

    // Quit the debugger
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that breakpoints inside function bodies are verified when the
/// function definition is evaluated, not when the function is called.
///
/// This tests the timing of verification events: when we source a file
/// containing a function with breakpoints inside it, those breakpoints
/// become verified as soon as R evaluates the function definition (the
/// `foo <- function() {...}` expression), before the function is called.
#[test]
fn test_dap_inner_breakpoint_verified_on_step() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create a file with a function containing a nested {} block.
    // The browser() stops us inside the function, allowing us to verify
    // that the breakpoint inside the nested block was already verified
    // when the function was defined (not when we step over it).
    //
    // Line numbers (1-indexed):
    // Line 1: (empty)
    // Line 2: foo <- function() {
    // Line 3:   browser()
    // Line 4:   {
    // Line 5:     1        <- BP here
    // Line 6:   }
    // Line 7: }
    // Line 8: foo()
    let file = SourceFile::new(
        "
foo <- function() {
  browser()
  {
    1
  }
}
foo()
",
    );

    // Set breakpoint on line 5 (the `1` expression inside nested {}) BEFORE sourcing
    let breakpoints = dap.set_breakpoints(&file.path, &[5]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Source the file - the function definition is evaluated and breakpoints are injected.
    // Then foo() is called which hits browser().
    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // The breakpoint gets verified when the function definition is evaluated.
    // This happens BEFORE we hit browser() inside the function call.
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(5));

    // Then we hit browser() and stop
    frontend.recv_iopub_async(vec![
        is_start_debug(),
        stream_contains("Called from:"),
        is_idle(),
    ]);
    frontend.recv_shell_execute_reply();
    dap.recv_stopped();

    // Verify we're stopped at browser() in foo
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "foo()");

    // Step with `n` to step over the inner {} block
    frontend.debug_send_step_command("n");
    dap.recv_continued();
    dap.recv_stopped();

    // Verify we're still in foo after stepping over the inner block
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "foo()");

    // Quit the debugger
    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Test stepping from one breakpoint onto an adjacent breakpoint.
///
/// When stopped at BP1 and stepping with `n` to a line with BP2, the auto-stepping
/// mechanism handles the injected breakpoint code transparently. The DAP event
/// sequence is more complex than a regular step because R steps through:
/// 1. `.ark_auto_step(...)` wrapper (detected via "debug at" message)
/// 2. `.ark_breakpoint(...)` function (detected via function class)
/// 3. Finally the actual user expression at BP2
///
/// Expected DAP events when stepping onto an adjacent breakpoint:
/// - Continued (from stop_debug after user's `n`)
/// - Stopped (at .ark_auto_step)
/// - Continued (auto-step over .ark_auto_step)
/// - Continued (from stop_debug)
/// - Stopped (in .ark_breakpoint)
/// - Continued (auto-step out of .ark_breakpoint)
/// - Continued (from stop_debug)
/// - Stopped (at BP2 user expression)
#[test]
fn test_dap_step_to_adjacent_breakpoint() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a function containing two adjacent breakpoints.
    // Line numbers (1-indexed):
    // Line 1: (empty)
    // Line 2: foo <- function() {
    // Line 3:   x <- 1  # BP1
    // Line 4:   y <- 2  # BP2
    // Line 5:   x + y
    // Line 6: }
    // Line 7: foo()
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

    // Set breakpoints on lines 3 and 4 BEFORE sourcing
    let breakpoints = dap.set_breakpoints(&file.path, &[3, 4]);
    assert_eq!(breakpoints.len(), 2);
    assert!(!breakpoints[0].verified);
    assert!(!breakpoints[1].verified);
    let bp1_id = breakpoints[0].id;
    let bp2_id = breakpoints[1].id;

    // Source the file - breakpoints get verified when function definition is evaluated
    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Both breakpoints become verified when the function definition is evaluated
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp1_id);
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp2_id);

    // Hit BP1: auto-stepping flow
    frontend.recv_iopub_breakpoint_hit();

    // DAP events for hitting BP1: auto-step through .ark_breakpoint wrapper,
    // then stop at user expression.
    dap.recv_auto_step_through();
    dap.recv_stopped();

    // Verify we're stopped at BP1 (line 3: x <- 1)
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "foo()");
    assert_eq!(stack[0].line, 3);

    // Step with `n` to BP2 - this is the key part of the test.
    // When stepping onto an injected breakpoint, we go through:
    // 1. .ark_auto_step wrapper
    // 2. .ark_breakpoint function
    // 3. Actual user expression
    frontend.send_execute_request("n", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // IOPub messages: stepping onto an adjacent breakpoint produces multiple
    // start_debug/stop_debug cycles due to auto-stepping through the injected code.
    // We expect 4 start_debug, 4 stop_debug, and idle (ordering not guaranteed).
    frontend.recv_iopub_until(|acc| {
        acc.has_comm_method_count("start_debug", 4) &&
            acc.has_comm_method_count("stop_debug", 4) &&
            acc.saw_idle()
    });

    frontend.recv_shell_execute_reply();

    // DAP events when stepping onto an adjacent breakpoint.
    // When stepping with `n` from BP1 onto BP2's injected code, R steps through
    // the .ark_auto_step and .ark_breakpoint wrappers with auto-stepping.
    //
    // Enable ARK_TEST_TRACE=all to see the actual message sequence:
    //   ARK_TEST_TRACE=all cargo nextest run test_dap_step_to_adjacent_breakpoint --success-output=immediate
    //
    // The sequence is: Continued (step starts), then auto-step through 3 wrappers
    // (.ark_auto_step, .ark_breakpoint, nested), then stop at BP2 user expression.
    dap.recv_continued();
    dap.recv_auto_step_through(); // .ark_auto_step wrapper
    dap.recv_auto_step_through(); // .ark_breakpoint wrapper
    dap.recv_auto_step_through(); // Nested wrapper
    dap.recv_stopped(); // At BP2 user expression (y <- 2)

    // Verify we're stopped at BP2 (line 4: y <- 2)
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "foo()");
    assert_eq!(stack[0].line, 4);

    // Quit the debugger. This triggers the cleanup in r_read_console which
    // sends a Continued event via stop_debug().
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that breakpoints inside `lapply()` callbacks hit on each iteration.
///
/// This is a key use case where users want to debug code that runs multiple times
/// in a loop construct. The breakpoint should be verified once when the callback
/// function is defined, and then hit on each iteration of lapply.
#[test]
fn test_dap_breakpoint_lapply_iteration() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with lapply calling a function with a breakpoint.
    // Line numbers (1-indexed):
    // Line 1: (empty)
    // Line 2: lapply(1:3, function(x) {
    // Line 3:   y <- x + 1  # BP here
    // Line 4:   y
    // Line 5: })
    let file = SourceFile::new(
        "
lapply(1:3, function(x) {
  y <- x + 1
  y
})
",
    );

    // Set breakpoint on line 3 (y <- x + 1) BEFORE sourcing
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Source the file - breakpoint gets verified when the anonymous function is evaluated
    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Breakpoint becomes verified when the function definition is evaluated
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(3));

    // First iteration: hit breakpoint with x=1
    frontend.recv_iopub_breakpoint_hit();
    dap.recv_auto_step_through();
    dap.recv_stopped();

    // Verify we're stopped at the breakpoint
    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 3);

    // Continue to second iteration: x=2.
    // Send `c` via Shell to continue execution. R will hit the breakpoint again
    // on the next iteration of lapply.
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // When continuing from inside lapply, the breakpoint is hit again.
    // The flow includes stop_debug (exiting current debug) and start_debug (new hit).
    // Note: idle timing relative to stop_debug is not guaranteed.
    frontend.recv_iopub_until(|acc| {
        acc.has_comm_method_count("start_debug", 2) &&
            acc.has_comm_method_count("stop_debug", 2) &&
            acc.saw_idle()
    });
    frontend.recv_shell_execute_reply();

    // DAP events: Continued from stop_debug, then auto-step through, then stopped
    dap.recv_continued();
    dap.recv_auto_step_through();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 3);

    // Continue to third iteration: x=3
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_until(|acc| {
        acc.has_comm_method_count("start_debug", 2) &&
            acc.has_comm_method_count("stop_debug", 2) &&
            acc.saw_idle()
    });
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
    dap.recv_auto_step_through();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 3);

    // Continue past the last iteration - execution completes normally
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // R exits the debugger and completes lapply (returns list result).
    // stop_debug is async, but execute_result must come before idle.
    frontend.recv_iopub_until(|acc| {
        acc.has_comm_method("stop_debug") && acc.in_order(&[is_execute_result(), is_idle()])
    });
    frontend.recv_shell_execute_reply();

    dap.recv_continued();

    // Receive the shell reply for the original source() request
    frontend.recv_shell_execute_reply();
}

/// Test that breakpoints inside a function defined within `local()` are hit
/// when the function is called.
///
/// This tests breakpoints in nested scopes, which is important for package
/// development where functions are often defined inside local() blocks.
#[test]
fn test_dap_breakpoint_nested_local_function() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a function defined inside local() that we call directly.
    // No browser() needed - we just source and let the breakpoint be hit.
    //
    // Line numbers (1-indexed):
    // Line 1: (empty)
    // Line 2: local({
    // Line 3:   inner_fn <- function() {
    // Line 4:     z <- 42  # BP here
    // Line 5:     z
    // Line 6:   }
    // Line 7:   inner_fn()
    // Line 8: })
    let file = SourceFile::new(
        "
local({
  inner_fn <- function() {
    z <- 42
    z
  }
  inner_fn()
})
",
    );

    // Set breakpoint on line 4 (z <- 42) BEFORE sourcing
    let breakpoints = dap.set_breakpoints(&file.path, &[4]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Source the file - the function is defined and called, hitting the breakpoint
    frontend.source_file_and_hit_breakpoint(&file);

    // Breakpoint is verified when hit
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(4));

    // Auto-step through wrapper and stop at user expression
    dap.recv_auto_step_through();
    dap.recv_stopped();

    // Verify we're stopped at the breakpoint (line 4: z <- 42)
    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 4);
    assert_eq!(stack[0].name, "inner_fn()");

    // Quit the debugger
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that stepping through `.ark_auto_step()` wrapper is transparent.
///
/// When a breakpoint is injected, it's wrapped in `.ark_auto_step({ .ark_breakpoint(...) })`.
/// Stepping over this wrapper should land on the next user expression, not inside the wrapper.
/// This test verifies the auto-stepping mechanism works correctly by checking that we
/// don't see intermediate stops inside the injected code.
#[test]
fn test_dap_auto_step_transparent() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create a file with a function containing multiple expressions.
    // We'll set a breakpoint on the first expression and verify that
    // after hitting the breakpoint, we're at the user expression (not inside wrappers).
    //
    // Line numbers (1-indexed):
    // Line 1: (empty)
    // Line 2: foo <- function() {
    // Line 3:   a <- 1  # BP here
    // Line 4:   b <- 2
    // Line 5:   a + b
    // Line 6: }
    // Line 7: foo()
    let file = SourceFile::new(
        "
foo <- function() {
  a <- 1
  b <- 2
  a + b
}
foo()
",
    );

    // Set breakpoint on line 3 (a <- 1)
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    let bp_id = breakpoints[0].id;

    // Source the file and hit the breakpoint
    frontend.source_file_and_hit_breakpoint(&file);

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);

    // The auto-step mechanism transparently steps through the wrapper functions
    // (.ark_auto_step and .ark_breakpoint) and stops at the user expression
    dap.recv_auto_step_through();
    dap.recv_stopped();

    // Verify we're at the user expression (line 3), not inside any wrapper
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "foo()");
    assert_eq!(stack[0].line, 3);

    // Quit the debugger
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that breakpoints inside R6 class methods work correctly.
///
/// R6 is a popular OOP system for R. This test verifies that breakpoints
/// can be set and hit inside R6 method definitions, which was mentioned
/// as an improvement over RStudio's debugging capabilities.
///
/// This test is skipped if the R6 package is not installed.
#[test]
fn test_dap_breakpoint_r6_method() {
    let frontend = DummyArkFrontend::lock();

    // Check if R6 is installed
    if !frontend.is_installed("R6") {
        println!("Skipping test_dap_breakpoint_r6_method: R6 package not installed");
        return;
    }

    let mut dap = frontend.start_dap();

    // Create file with an R6 class that has a method with a breakpoint.
    //
    // Line numbers (1-indexed):
    // Line 1: (empty)
    // Line 2: Counter <- R6::R6Class("Counter",
    // Line 3:   public = list(
    // Line 4:     count = 0,
    // Line 5:     increment = function() {
    // Line 6:       self$count <- self$count + 1  # BP here
    // Line 7:       self$count
    // Line 8:     }
    // Line 9:   )
    // Line 10: )
    // Line 11: c <- Counter$new()
    // Line 12: c$increment()
    let file = SourceFile::new(
        r#"
Counter <- R6::R6Class("Counter",
  public = list(
    count = 0,
    increment = function() {
      self$count <- self$count + 1
      self$count
    }
  )
)
c <- Counter$new()
c$increment()
"#,
    );

    // Set breakpoint on line 6 (self$count <- self$count + 1) BEFORE sourcing
    let breakpoints = dap.set_breakpoints(&file.path, &[6]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Source the file - the R6 class is defined, an instance created, and method called
    frontend.source_file_and_hit_breakpoint(&file);

    // Breakpoint is verified when the method is hit
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(6));

    // Auto-step through wrapper and stop at user expression
    dap.recv_auto_step_through();
    dap.recv_stopped();

    // Verify we're stopped at the breakpoint inside the R6 method
    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 6);
    // The method name includes the class context
    assert!(
        stack[0].name.contains("increment"),
        "Expected stack frame name to contain 'increment', got: {}",
        stack[0].name
    );

    // Quit the debugger
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that the source hook falls back to regular source() when disabled.
///
/// When `ark.source_hook` option is FALSE, the custom source() hook should
/// fall back to R's original source() function, meaning breakpoints won't
/// be injected and verified during sourcing.
#[test]
fn test_dap_source_hook_fallback_when_disabled() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a function containing a breakpoint location
    let file = SourceFile::new(
        "
foo <- function() {
  x <- 1
  x + 1
}
",
    );

    // Set breakpoint BEFORE sourcing
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);

    // Disable the source hook
    frontend.execute_request_invisibly("options(ark.source_hook = FALSE)");

    // Source the file - with hook disabled, breakpoint should NOT be verified
    frontend.send_execute_request(
        &format!("source('{}')", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // No breakpoint event should have been sent
    dap.assert_no_events();

    // Re-enable the source hook for cleanup
    frontend.execute_request_invisibly("options(ark.source_hook = TRUE)");

    // Now source again - breakpoint should be verified this time
    frontend.source_file(&file);

    // Breakpoint becomes verified when the function definition is evaluated
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.line, Some(3));
}

/// Test that the source hook falls back when unsupported arguments are passed.
///
/// The source hook only handles the `file`, `echo`, and `local` arguments.
/// When other arguments are passed (like `chdir`, `print.eval`, etc.),
/// it should fall back to R's original source() function.
#[test]
fn test_dap_source_hook_fallback_with_extra_args() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a function containing a breakpoint location
    let file = SourceFile::new(
        "
bar <- function() {
  y <- 2
  y + 2
}
",
    );

    // Set breakpoint BEFORE sourcing
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);

    // Source with an extra argument (chdir) - this should trigger fallback
    frontend.send_execute_request(
        &format!("source('{}', chdir = TRUE)", file.path),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // No breakpoint event should have been sent due to fallback
    dap.assert_no_events();

    // Verify the function was still defined (fallback worked)
    frontend.execute_request_invisibly("stopifnot(exists('bar'))");
}

/// Test that breakpoints inside `for` loops hit on each iteration.
///
/// Similar to the lapply test, but for traditional for loops.
/// The breakpoint should hit on each iteration of the loop.
#[test]
fn test_dap_breakpoint_for_loop_iteration() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a for loop containing a breakpoint.
    // Line numbers (1-indexed):
    // Line 1: (empty)
    // Line 2: {
    // Line 3:   for (i in 1:3) {
    // Line 4:     x <- i * 2  # BP here
    // Line 5:   }
    // Line 6: }
    let file = SourceFile::new(
        "
{
  for (i in 1:3) {
    x <- i * 2
  }
}
",
    );

    // Set breakpoint on line 4 (x <- i * 2) BEFORE sourcing
    let breakpoints = dap.set_breakpoints(&file.path, &[4]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Source the file - breakpoint gets verified when the braced block is evaluated
    frontend.source_file_and_hit_breakpoint(&file);

    // Breakpoint becomes verified
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(4));

    // First iteration: i=1
    dap.recv_auto_step_through();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 4);

    // Continue to second iteration: i=2
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    // Note: idle timing relative to stop_debug is not guaranteed.
    frontend.recv_iopub_until(|acc| {
        acc.has_comm_method_count("start_debug", 2) &&
            acc.has_comm_method_count("stop_debug", 2) &&
            acc.saw_idle()
    });
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
    dap.recv_auto_step_through();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 4);

    // Continue to third iteration: i=3
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_until(|acc| {
        acc.has_comm_method_count("start_debug", 2) &&
            acc.has_comm_method_count("stop_debug", 2) &&
            acc.saw_idle()
    });
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
    dap.recv_auto_step_through();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 4);

    // Continue past the last iteration - execution completes.
    // Use stream-skipping variants because late-arriving debug output
    // from previous iterations can interleave here.
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy_skip_streams();
    frontend.recv_iopub_execute_input_skip_streams();
    // stop_debug is async, but execute_result must come before idle.
    frontend.recv_iopub_until(|acc| {
        acc.has_comm_method("stop_debug") && acc.in_order(&[is_execute_result(), is_idle()])
    });
    frontend.recv_shell_execute_reply();

    dap.recv_continued();

    // Receive the shell reply for the original source() request
    frontend.recv_shell_execute_reply();
}

/// Test that breakpoints inside tryCatch error handlers work correctly.
///
/// This verifies that breakpoints can be hit inside error handling code,
/// which is important for debugging error recovery logic.
#[test]
fn test_dap_breakpoint_trycatch_handler() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a tryCatch that has a breakpoint in the error handler.
    // Line numbers (1-indexed):
    // Line 1: (empty)
    // Line 2: result <- tryCatch({
    // Line 3:   stop("test error")
    // Line 4: }, error = function(e) {
    // Line 5:   msg <- conditionMessage(e)  # BP here
    // Line 6:   paste("Caught:", msg)
    // Line 7: })
    let file = SourceFile::new(
        r#"
result <- tryCatch({
  stop("test error")
}, error = function(e) {
  msg <- conditionMessage(e)
  paste("Caught:", msg)
})
"#,
    );

    // Set breakpoint on line 5 (msg <- conditionMessage(e)) BEFORE sourcing
    let breakpoints = dap.set_breakpoints(&file.path, &[5]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // Source the file - the error is thrown and caught, triggering the handler
    frontend.source_file_and_hit_breakpoint(&file);

    // Breakpoint is verified when hit in the error handler
    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);
    assert_eq!(bp.line, Some(5));

    // Auto-step through wrapper and stop at user expression
    dap.recv_auto_step_through();
    dap.recv_stopped();

    // Verify we're stopped at the breakpoint inside the error handler
    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 5);

    // Quit the debugger
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}

/// Test that breakpoints work correctly when the same file is sourced multiple times.
///
/// Re-sourcing a file should re-verify breakpoints as the code is re-parsed.
#[test]
fn test_dap_breakpoint_multiple_source_same_file() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Create file with a simple function
    let file = SourceFile::new(
        "
greet <- function(name) {
  msg <- paste('Hello,', name)
  msg
}
greet('World')
",
    );

    // Set breakpoint on line 3 BEFORE first source
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);
    assert!(!breakpoints[0].verified);
    let bp_id = breakpoints[0].id;

    // First source - breakpoint gets verified and hit
    frontend.source_file_and_hit_breakpoint(&file);

    let bp = dap.recv_breakpoint_verified();
    assert_eq!(bp.id, bp_id);

    dap.recv_auto_step_through();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 3);
    assert_eq!(stack[0].name, "greet()");

    // Quit the debugger to complete first source
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();

    // Second source of the same file - breakpoint should hit again
    frontend.source_file_and_hit_breakpoint(&file);

    // No new verification event needed - breakpoint is already verified
    dap.recv_auto_step_through();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 3);
    assert_eq!(stack[0].name, "greet()");

    // Quit and finish
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}
