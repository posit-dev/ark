//
// dap_exception_breakpoints.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::thread;
use std::time::Duration;

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark::sys::control::handle_interrupt_request;
use ark_test::DummyArkFrontend;
use ark_test::SourceFile;

/// Test that an error at top-level triggers the debugger when error breakpoints are enabled
#[test]
fn test_dap_break_on_error() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    dap.set_exception_breakpoints(&["error"]);

    frontend.send_source("stop('test error')");

    let (text, description) = dap.recv_stopped_exception();
    assert!(text.contains("simpleError"));
    assert!(description.contains("test error"));

    let stack = dap.stack_trace();
    assert!(!stack.is_empty());

    // Continue out of debugger - error propagates
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_execute_error();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();

    dap.recv_continued();
}

/// Test that an error from nested function calls (f -> g -> h -> stop) triggers
/// the debugger and shows the full call stack
#[test]
fn test_dap_break_on_error_nested_calls() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    dap.set_exception_breakpoints(&["error"]);

    let file = frontend.send_source(
        "
f <- function() g()
g <- function() h()
h <- function() stop('nested error')
f()
",
    );

    let (text, description) = dap.recv_stopped_exception();
    assert!(text.contains("simpleError"));
    assert!(description.contains("nested error"));

    // Stack should contain the user functions h, g, f (plus R internal frames)
    let stack = dap.stack_trace();
    let frame_names: Vec<&str> = stack.iter().map(|f| f.name.as_str()).collect();

    assert!(
        frame_names.contains(&"h()"),
        "Expected h() in stack, got: {:?}",
        frame_names
    );
    assert!(
        frame_names.contains(&"g()"),
        "Expected g() in stack, got: {:?}",
        frame_names
    );
    assert!(
        frame_names.contains(&"f()"),
        "Expected f() in stack, got: {:?}",
        frame_names
    );

    // Verify at least one frame has source location pointing to the file
    let has_source = stack.iter().any(|f| {
        f.source
            .as_ref()
            .is_some_and(|s| s.path.as_ref().is_some_and(|p| p.contains(&file.filename)))
    });
    assert!(has_source, "Expected at least one frame with source info");

    // Continue out of debugger - error propagates
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_execute_error();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();

    dap.recv_continued();
}

/// Test that the global error handler frame is excluded from the stack trace
#[test]
fn test_dap_break_on_error_excludes_handler_frame() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    dap.set_exception_breakpoints(&["error"]);

    frontend.send_source(
        "
f <- function() stop('test')
f()
",
    );

    dap.recv_stopped_exception();

    let stack = dap.stack_trace();
    let frame_names: Vec<&str> = stack.iter().map(|f| f.name.as_str()).collect();

    // The global error handler frame (named "h()" by R's calling handler machinery)
    // should be excluded from the stack
    assert_ne!(
        frame_names[0], "h()",
        "Handler frame 'h()' should be excluded, got: {:?}",
        frame_names
    );

    // But user frames should still be present
    assert!(
        frame_names.contains(&"f()"),
        "Expected f() in stack, got: {:?}",
        frame_names
    );

    // Continue out of debugger
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_execute_error();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();

    dap.recv_continued();
}

/// Test that `.handleSimpleError()` frame is excluded from the stack trace
#[test]
fn test_dap_break_on_error_excludes_handle_simple_error_frame() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    dap.set_exception_breakpoints(&["error"]);

    frontend.send_source("stop('test')");

    dap.recv_stopped_exception();

    let stack = dap.stack_trace();
    let frame_names: Vec<&str> = stack.iter().map(|f| f.name.as_str()).collect();

    // The `.handleSimpleError()` frame should be excluded from the stack
    assert_ne!(
        frame_names[0], ".handleSimpleError()",
        "Handler frame '.handleSimpleError()' should be excluded, got: {:?}",
        frame_names
    );

    // Continue out of debugger
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_execute_error();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();

    dap.recv_continued();
}

/// Test that warning machinery frames are excluded from the stack trace
#[test]
fn test_dap_break_on_warning_excludes_signal_simple_warning_frames() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    dap.set_exception_breakpoints(&["warning"]);

    frontend.send_source("warning('test')");

    dap.recv_stopped_exception();

    let stack = dap.stack_trace();
    let frame_names: Vec<&str> = stack.iter().map(|f| f.name.as_str()).collect();

    // The warning machinery frames should be excluded from the stack
    let warning_frames = [
        "doWithOneRestart()",
        "withOneRestart()",
        "withRestarts()",
        ".signalSimpleWarning",
    ];
    for frame in warning_frames {
        assert!(
            !frame_names.iter().any(|f| f.starts_with(frame)),
            "Frame '{frame}' should be excluded, got: {frame_names:?}",
        );
    }

    // Continue out of debugger
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.assert_stream_stderr_contains("test");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
}

/// Test that the global warning handler frame is excluded from the stack trace
#[test]
fn test_dap_break_on_warning_excludes_handler_frame() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    dap.set_exception_breakpoints(&["warning"]);

    frontend.send_source(
        "
f <- function() warning('test')
f()
",
    );

    dap.recv_stopped_exception();

    let stack = dap.stack_trace();
    let frame_names: Vec<&str> = stack.iter().map(|f| f.name.as_str()).collect();

    // The global warning handler frame (an anonymous function) should be excluded
    // from the stack
    assert!(
        !frame_names[0].starts_with("(function"),
        "Handler frame should be excluded, got: {:?}",
        frame_names
    );

    // User frames should still be present
    assert!(
        frame_names.contains(&"f()"),
        "Expected f() in stack, got: {:?}",
        frame_names
    );

    // Continue out of debugger
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.assert_stream_stderr_contains("test");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
}

/// Test that a warning triggers the debugger when warning breakpoints are enabled
#[test]
fn test_dap_break_on_warning() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    dap.set_exception_breakpoints(&["warning"]);

    frontend.send_source("warning('test warning')");

    let (text, description) = dap.recv_stopped_exception();
    assert!(text.contains("simpleWarning"));
    assert!(description.contains("test warning"));

    let stack = dap.stack_trace();
    assert!(!stack.is_empty());

    // Continue out of debugger - warning message printed to stderr
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.assert_stream_stderr_contains("test warning");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
}

/// Test that a warning from nested function calls triggers the debugger
#[test]
fn test_dap_break_on_warning_nested_calls() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    dap.set_exception_breakpoints(&["warning"]);

    let file = frontend.send_source(
        "
f <- function() g()
g <- function() h()
h <- function() warning('nested warning')
f()
",
    );

    let (text, description) = dap.recv_stopped_exception();
    assert!(text.contains("simpleWarning"));
    assert!(description.contains("nested warning"));

    // Stack should contain the user functions h, g, f
    let stack = dap.stack_trace();
    let frame_names: Vec<&str> = stack.iter().map(|f| f.name.as_str()).collect();

    assert!(
        frame_names.contains(&"h()"),
        "Expected h() in stack, got: {:?}",
        frame_names
    );
    assert!(
        frame_names.contains(&"g()"),
        "Expected g() in stack, got: {:?}",
        frame_names
    );
    assert!(
        frame_names.contains(&"f()"),
        "Expected f() in stack, got: {:?}",
        frame_names
    );

    // Verify at least one frame has source location pointing to the file
    let has_source = stack.iter().any(|f| {
        f.source
            .as_ref()
            .is_some_and(|s| s.path.as_ref().is_some_and(|p| p.contains(&file.filename)))
    });
    assert!(has_source, "Expected at least one frame with source info");

    // Continue out of debugger - warning message printed to stderr
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.assert_stream_stderr_contains("nested warning");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
}

/// Test that disabling exception breakpoints stops triggering the debugger
#[test]
fn test_dap_disable_exception_breakpoints() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Enable break on error
    dap.set_exception_breakpoints(&["error"]);

    // First error should break
    frontend.send_source("stop('first')");
    dap.recv_stopped_exception();

    // Continue out of debugger
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_execute_error();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();

    dap.recv_continued();

    // Disable exception breakpoints
    dap.set_exception_breakpoints(&[]);

    // Second error should NOT break - just execute normally
    frontend.execute_request_error("stop('second')", |msg| {
        assert!(msg.contains("second"));
    });
}

/// Test that pause interrupts execution and drops into the debugger
#[test]
fn test_dap_pause() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Define a function with an infinite loop
    frontend.send_execute_request(
        "looper <- function() { repeat NULL }",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Start the infinite loop
    frontend.send_execute_request("looper()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Give R a moment to enter the loop
    thread::sleep(Duration::from_millis(30));

    // Send pause request
    dap.pause();

    // Should receive start_debug and stopped event
    frontend.recv_iopub_start_debug();
    dap.recv_stopped();

    // Verify we're stopped inside looper()
    // Stack has: looper(), <global> (interrupt handler frame is excluded)
    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 2);
    assert_eq!(stack[0].name, "looper()");

    // The pause completed, receive idle before sending Q
    frontend.assert_stream_stdout_contains("Called from:");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Quit the debugger (Q exits and propagates the interrupt)
    frontend.send_execute_request("Q", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
}

/// Test that pause works correctly even when the interrupt is caught by tryCatch.
/// When tryCatch(interrupt = ) catches the interrupt before our global calling handler,
/// the `is_interrupting_for_debugger` flag could remain set, causing the next regular
/// interrupt to incorrectly drop into the debugger. This test verifies the flag is
/// properly reset as a fallback when returning to the top-level prompt.
#[test]
fn test_dap_pause_with_trycatch_interrupt_handler() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Start code that uses tryCatch to swallow interrupts
    frontend.send_execute_request(
        "tryCatch({ Sys.sleep(10) }, interrupt = function(e) 'caught')",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Give R a moment to enter the sleep
    thread::sleep(Duration::from_millis(30));

    // Send pause request - but tryCatch will catch the interrupt before
    // our global calling handler can process it
    dap.pause();

    // The interrupt is caught by tryCatch, so we don't enter the debugger.
    // We should get the result 'caught' back.
    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Now the critical test: send a REGULAR interrupt (not a pause) to code
    // WITHOUT tryCatch. If the `is_interrupting_for_debugger` flag wasn't reset,
    // this regular interrupt would incorrectly be treated as a debugger pause.
    frontend.send_execute_request("Sys.sleep(10)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    thread::sleep(Duration::from_millis(30));

    // Send a regular interrupt (NOT pause) - this simulates Ctrl+C
    handle_interrupt_request();

    // The interrupt should propagate normally (no tryCatch to catch it).
    // If the flag wasn't reset, we'd incorrectly enter the debugger here.
    // Instead, we should just get the interrupt error.
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Verify DAP didn't receive any stopped event (we should NOT have entered debugger)
    dap.assert_no_events();
}

/// Test that an error thrown while already paused at a regular breakpoint
/// correctly trims the handler frame from the stack trace.
#[test]
fn test_dap_break_on_error_while_at_breakpoint() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Enable error exception breakpoints
    dap.set_exception_breakpoints(&["error"]);

    let file = SourceFile::new(
        "
foo <- function() {
  x <- 1
  x + 1
}
foo()
",
    );

    // Set breakpoint on line 3: `x <- 1`
    let breakpoints = dap.set_breakpoints(&file.path, &[3]);
    assert_eq!(breakpoints.len(), 1);

    // Source the file to hit the breakpoint
    frontend.source_file_and_hit_breakpoint(&file);
    dap.recv_breakpoint_verified();
    dap.recv_stopped();

    // Verify we're stopped at the breakpoint inside foo()
    dap.assert_top_frame("foo()");

    // While paused at the breakpoint, evaluate code that triggers an error.
    // The error exception breakpoint should fire and create a nested browser.
    frontend.send_execute_request("stop('nested error')", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();

    // Leaving the breakpoint debug session emits a Continued event
    dap.recv_continued();

    frontend.recv_iopub_start_debug();

    // Should receive a stopped *exception* event (not a step event).
    // This verifies `debug_stopped_reason` was correctly set to `Condition`.
    let (text, description) = dap.recv_stopped_exception();
    assert!(text.contains("simpleError"));
    assert!(description.contains("nested error"));

    // The stream output shows where the browser was called from
    frontend.assert_stream_stdout_contains("Called from:");

    // The kernel goes idle while waiting for input in the error browser
    frontend.recv_iopub_idle();

    // The handler frame should be trimmed from the stack
    let stack = dap.stack_trace();
    let frame_names: Vec<&str> = stack.iter().map(|f| f.name.as_str()).collect();

    // foo() should be in the stack (we errored while inside it)
    assert!(
        frame_names.contains(&"foo()"),
        "Expected foo() in stack, got: {frame_names:?}",
    );

    // Continue out of the error browser. `globalErrorHandler`'s defer block
    // saves the traceback, invokes `options(error)`, and calls
    // `invokeRestart("abort")` which jumps to top level.
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    dap.recv_continued();

    // The abort restart jumps to top level. The error is reported with a
    // proper traceback (saved by `globalErrorHandler`).
    let evalue = frontend.recv_iopub_execute_error();
    assert!(evalue.contains("nested error"));
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply(); // source()
    frontend.recv_shell_execute_reply(); // stop('nested error')
    frontend.recv_shell_execute_reply_exception(); // c
}
