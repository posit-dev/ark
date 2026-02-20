//
// lsp.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

// The lsp files in ark_test are for only integration tests with the Jupyter
// kernel, i.e. LSP features that require dynamic access to the R session.

use ark_test::DummyArkFrontend;

#[test]
fn test_lsp_init() {
    let frontend = DummyArkFrontend::lock();
    let lsp = frontend.start_lsp();

    // Verify the server reports completion support
    assert!(lsp.server_capabilities().completion_provider.is_some());
}
