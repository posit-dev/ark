//
// dap_breakpoints_integrations.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;
use ark_test::SourceFile;

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

    // Direct function call - recv_iopub_breakpoint_hit handles the debug message flow
    frontend.recv_iopub_breakpoint_hit();

    dap.recv_stopped();

    // Verify we're stopped at the right place
    dap.assert_top_frame("foo()");
    dap.assert_top_frame_line(3);
    dap.assert_top_frame_file(&file);

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
    dap.recv_stopped();

    // Verify we're stopped at the breakpoint inside the R6 method
    dap.assert_top_frame_line(6);
    dap.assert_top_frame_file(&file);
    let stack = dap.stack_trace();
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

    dap.recv_stopped();

    dap.assert_top_frame("greet()");
    dap.assert_top_frame_line(3);
    dap.assert_top_frame_file(&file);

    // Quit the debugger to complete first source
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();

    // Second source of the same file - breakpoint should hit again
    frontend.source_file_and_hit_breakpoint(&file);

    // No new verification event needed - breakpoint is already verified
    dap.recv_stopped();

    dap.assert_top_frame("greet()");
    dap.assert_top_frame_line(3);
    dap.assert_top_frame_file(&file);

    // Quit and finish
    frontend.debug_send_quit();
    dap.recv_continued();
    frontend.recv_shell_execute_reply();
}
