//
// dap_exception_breakpoints.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::thread;
use std::time::Duration;

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

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
    // Stack has: interrupt handler, looper(), <global>
    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 3);
    assert_eq!(stack[1].name, "looper()");

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
