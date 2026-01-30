//
// dap.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use ark_test::DummyArkFrontend;
use dap::types::StackFrame;
use dap::types::Thread;

#[test]
fn test_dap_initialize_and_disconnect() {
    let frontend = DummyArkFrontend::lock();

    // `start_dap()` connects and initializes, `Drop` disconnects
    let mut dap = frontend.start_dap();

    // First thing sent by frontend after connection
    assert!(matches!(
        dap.threads().as_slice(),
        [Thread { id: -1, name }] if name == "R console"
    ));
}

#[test]
fn test_dap_stopped_at_browser() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();

    dap.recv_stopped();

    assert!(matches!(
        dap.stack_trace().as_slice(),
        [StackFrame { name, line: 1, .. }] if name == "<global>"
    ));

    frontend.debug_send_quit();
    dap.recv_continued();
}
