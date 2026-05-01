//
// kernel-stream-capture.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
// Integration tests for fd-level stream capture (`StreamCapture`).
//
// R's own output goes through `WriteConsole` callbacks, but child processes
// spawned by `system()` write directly to the inherited file descriptors.
// `StreamCapture` redirects stdout/stderr into pipes so that output is
// forwarded to the frontend via IOPub.
//
// These tests use `DummyArkFrontendStreamCapture` which enables stream
// capture. Because the redirect affects global file descriptors, panic
// messages won't be visible in test runner output.

// Stream capture is currently not supported on Windows
#![cfg(unix)]

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontendStreamCapture;

#[test]
fn test_system_stdout_captured() {
    let frontend = DummyArkFrontendStreamCapture::lock();

    frontend.send_execute_request(
        "system('echo hello_from_system')",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stdout_contains("hello_from_system");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_system_stderr_captured() {
    let frontend = DummyArkFrontendStreamCapture::lock();

    frontend.send_execute_request(
        "system('echo error_from_system >&2')",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stderr_contains("error_from_system");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}
