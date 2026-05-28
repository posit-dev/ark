//
// dap_breakpoints_reconnect.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::thread;
use std::time::Duration;

use ark_test::DapClient;
use ark_test::DummyArkFrontend;
use ark_test::SourceFile;

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

    let mut file = SourceFile::new(
        "foo <- function() {
  x <- 1
  y <- 2
}
",
    );

    // Set breakpoint and record ID (unverified since we're using temp file)
    let breakpoints = dap.set_breakpoints(&file.path, &[2]);
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
    file.rewrite("bar <- function() {\n  a <- 10\n  b <- 20\n}\n");

    // Reconnect to the same DAP server (with retry since server needs time to accept)
    let mut dap = DapClient::connect("127.0.0.1", port).unwrap();
    dap.initialize();
    dap.attach();

    // Re-query breakpoints - state should be reset due to hash change
    let breakpoints = dap.set_breakpoints(&file.path, &[2]);
    assert_eq!(breakpoints.len(), 1);

    // ID should be different (state was discarded due to hash change)
    assert_ne!(breakpoints[0].id, original_id);

    // Breakpoint should be unverified
    assert!(!breakpoints[0].verified);
}
