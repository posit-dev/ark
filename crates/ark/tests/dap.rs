//
// dap.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use ark::fixtures::DummyArkFrontend;

#[test]
fn test_dap_initialize_and_disconnect() {
    let frontend = DummyArkFrontend::lock();

    // `start_dap()` connects and initializes, `Drop` disconnects
    let _dap = frontend.start_dap();
}
