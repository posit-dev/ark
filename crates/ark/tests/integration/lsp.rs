//
// lsp.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

// The lsp files in ark_test are for only integration tests with the Jupyter
// kernel, i.e. LSP features that require dynamic access to the R session.

use ark_test::DummyArkFrontend;
use serde_json::json;

#[test]
fn test_lsp_init() {
    let frontend = DummyArkFrontend::lock();
    let lsp = frontend.start_lsp();

    // Verify the server reports completion support
    assert!(lsp.server_capabilities().completion_provider.is_some());
}

// The two cases below test errors that don't depend on the rename
// implementation's resolution capabilities. New-name validation always
// applies (R language constraints), so these tests stay valid once
// cross-file rename lands.
//
// They also pin the wire format: an exact `assert_eq!` catches both
// `Anyhow(...)` wrapping and `Stack backtrace:` blocks that anyhow's
// `{:?}` formatting would smuggle into the editor popup.

#[test]
fn test_rename_to_reserved_word_returns_clean_error() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("rename_reserved.R", "foo <- 1\n");

    let params = json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": 0 },
        "newName": "if",
    });
    let message = lsp.send_request_expect_error("textDocument/rename", params);

    assert_eq!(message, "`if` is a reserved word in R");
}

#[test]
fn test_rename_to_empty_name_returns_clean_error() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("rename_empty.R", "foo <- 1\n");

    let params = json!({
        "textDocument": { "uri": uri },
        "position": { "line": 0, "character": 0 },
        "newName": "",
    });
    let message = lsp.send_request_expect_error("textDocument/rename", params);

    assert_eq!(message, "Identifier cannot be empty");
}
