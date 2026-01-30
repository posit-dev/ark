//
// dap.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use ark_test::DummyArkFrontend;

#[test]
fn test_dap_initialize_and_disconnect() {
    let frontend = DummyArkFrontend::lock();

    // `start_dap()` connects and initializes, `Drop` disconnects
    let _dap = frontend.start_dap();
}

#[test]
fn test_dap_stopped_at_browser() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}
