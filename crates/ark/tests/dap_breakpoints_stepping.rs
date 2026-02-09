//
// dap_breakpoints_stepping.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;
use ark_test::SourceFile;

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

    dap.recv_stopped();

    // Quit and finish
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
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

    // Hit browser() and stop
    frontend.recv_iopub_start_debug();
    frontend.assert_stream_stdout_contains("Called from:");
    frontend.recv_iopub_idle();
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

    // Hit BP1: auto-stepping is now transparent (no start_debug/stop_debug cycles)
    frontend.recv_iopub_breakpoint_hit();

    // DAP event: stopped at user expression (auto-stepping is transparent)
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

    // Stepping from BP1 to BP2 exits the current browser and enters a new one
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();

    // Drain the "debug at" stream output from auto-stepping
    frontend.drain_streams();

    frontend.recv_iopub_idle();

    frontend.recv_shell_execute_reply();

    // DAP events when stepping onto an adjacent breakpoint.
    // Auto-stepping through injected code is now transparent - we only see
    // the Continued when stepping starts and Stopped at the final destination.
    dap.recv_continued();
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
    dap.recv_stopped();

    // Verify we're stopped at the breakpoint
    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 3);

    // Continue to second iteration: x=2
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Hit the breakpoint again on next iteration (auto-stepping is now transparent)
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();
    frontend.assert_stream_stdout_contains("Called from:");
    frontend.assert_stream_stdout_contains("debug at");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // DAP events: Continued from stop_debug, then auto-step through, then stopped
    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 3);

    // Continue to third iteration: x=3
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    // Same pattern as second iteration
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();
    frontend.assert_stream_stdout_contains("Called from:");
    frontend.assert_stream_stdout_contains("debug at");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 3);

    // Continue past the last iteration - execution completes normally
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // R exits the debugger and completes lapply
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
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
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 4);

    // Continue to second iteration: i=2
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    // Hit the breakpoint again on next iteration (auto-stepping is now transparent)
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();
    frontend.assert_stream_stdout_contains("Called from:");
    frontend.assert_stream_stdout_contains("debug at");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 4);

    // Continue to third iteration: i=3
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    // Same pattern as second iteration
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();
    frontend.assert_stream_stdout_contains("Called from:");
    frontend.assert_stream_stdout_contains("debug at");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 4);

    // Continue past the last iteration
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    // R exits the debugger and completes the for loop
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
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
    dap.recv_stopped();

    // Verify we're stopped at the breakpoint inside the error handler
    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 5);

    // Quit the debugger
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}
