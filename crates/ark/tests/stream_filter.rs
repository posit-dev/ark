//
// stream_filter.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
// Integration tests verifying that debug messages are filtered from console output.
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

/// Verify that "Called from:" is filtered from console output when browser() is called.
#[test]
fn test_called_from_filtered_at_top_level() {
    let frontend = DummyArkFrontend::lock();

    // Execute browser() which would normally print "Called from: top level"
    frontend.send_execute_request("browser()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Drain any streams - should NOT contain "Called from:"
    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout.contains("Called from:"),
        "Called from: should be filtered from stdout, got: {:?}",
        streams.stdout
    );

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Exit the debugger
    frontend.execute_request_invisibly("Q");
}

/// Verify that "Called from:" is filtered when browser() is called inside a function.
#[test]
fn test_called_from_filtered_in_function() {
    let frontend = DummyArkFrontend::lock();

    // Define and call a function with browser()
    let code = "local({ browser() })";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Drain streams - should NOT contain "Called from:"
    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout.contains("Called from:"),
        "Called from: should be filtered from stdout, got: {:?}",
        streams.stdout
    );

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Exit the debugger
    frontend.execute_request_invisibly("Q");
}

/// Verify that "debug at" is filtered when stepping through sourced code.
#[test]
fn test_debug_at_filtered_when_stepping() {
    let frontend = DummyArkFrontend::lock();
    let _dap = frontend.start_dap();

    // Source a file with browser() to enter debug mode
    let file = frontend.send_source(
        "
{
  browser()
  1
  2
}
",
    );

    // The send_source helper already verified we entered debug mode.
    // Now step with `n` which would normally print "debug at file#line: expr"
    frontend.send_execute_request("n", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();

    // Drain streams - should NOT contain "debug at"
    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout.contains("debug at"),
        "debug at should be filtered from stdout, got: {:?}",
        streams.stdout
    );
    assert!(
        !streams.stdout.contains(&file.filename),
        "filename in debug message should be filtered, got: {:?}",
        streams.stdout
    );

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Exit the debugger
    frontend.debug_send_quit();
}

/// Verify that "debugging in:" and "exiting from:" are filtered.
/// This test uses debug() on a simple function to trigger both messages.
#[test]
fn test_debugging_in_and_exiting_from_filtered() {
    let frontend = DummyArkFrontend::lock();

    // Define a function and debug it
    frontend.execute_request_invisibly("f <- function() 42");
    frontend.execute_request_invisibly("debug(f)");

    // Call the function - this triggers "debugging in:"
    frontend.send_execute_request("f()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Drain streams at this point - should NOT contain "debugging in:"
    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout.contains("debugging in:"),
        "debugging in: should be filtered from stdout, got: {:?}",
        streams.stdout
    );

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Continue to exit the function - this triggers "exiting from:"
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Drain streams - should NOT contain "exiting from:"
    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout.contains("exiting from:"),
        "exiting from: should be filtered from stdout, got: {:?}",
        streams.stdout
    );

    // The result [1] 42 should come through
    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Clean up
    frontend.execute_request_invisibly("undebug(f)");
}

/// Verify that normal output is NOT filtered (sanity check).
#[test]
fn test_normal_output_not_filtered() {
    let frontend = DummyArkFrontend::lock();

    // Execute something that produces normal output
    frontend.send_execute_request("cat('Hello, World!\\n')", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Should see the normal output
    frontend.assert_stream_stdout_contains("Hello, World!");

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Verify that output during debug sessions that isn't debug chatter passes through.
#[test]
fn test_user_output_in_debug_not_filtered() {
    let frontend = DummyArkFrontend::lock();

    // Enter browser
    frontend.send_execute_request("browser()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Print something in the debug session
    frontend.send_execute_request(
        "cat('User output in debug\\n')",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // User output should NOT be filtered
    frontend.assert_stream_stdout_contains("User output in debug");

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Exit
    frontend.execute_request_invisibly("Q");
}
