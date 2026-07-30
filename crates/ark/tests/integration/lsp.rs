//
// lsp.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

// The lsp files in ark_test are for only integration tests with the Jupyter
// kernel, i.e. LSP features that require dynamic access to the R session.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use ark_test::DummyArkFrontend;
use serde_json::json;

#[test]
fn test_lsp_init() {
    let frontend = DummyArkFrontend::lock();
    let lsp = frontend.start_lsp();

    // Verify the server reports completion support
    assert!(lsp.server_capabilities().completion_provider.is_some());
}

// Reproduces https://github.com/posit-dev/ark/issues/1361: an abrupt client
// disconnect (TCP reset, e.g. the extension host being recycled) hits an
// `unreachable!()` in our pinned tower-lsp fork's transport and panics the
// `ark-lsp` thread.
#[test]
fn test_lsp_survives_abrupt_disconnect() {
    // The panic happens on the `ark-lsp` thread, so a plain `#[should_panic]`
    // or `catch_unwind` on this (the main test) thread can't see it. Install
    // a hook that records panics by thread name instead, and chain to the
    // previous hook so panic output still gets printed.
    let panic_message: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let panic_message_hook = Arc::clone(&panic_message);
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if std::thread::current().name() == Some("ark-lsp") {
            *panic_message_hook.lock().unwrap() = Some(info.to_string());
        }
        previous_hook(info);
    }));

    let frontend = DummyArkFrontend::lock();
    let lsp = frontend.start_lsp();

    lsp.disconnect_abruptly();

    // Give the LSP thread a moment to observe the reset.
    std::thread::sleep(Duration::from_millis(200));

    if let Some(message) = panic_message.lock().unwrap().take() {
        panic!("`ark-lsp` thread panicked on abrupt disconnect: {message}");
    }

    // The LSP should still be usable after a client disconnects abruptly.
    let lsp2 = frontend.start_lsp();
    assert!(lsp2.server_capabilities().completion_provider.is_some());
}

// Reproduces https://github.com/ebkalderon/tower-lsp/issues/399 and #424:
// some clients send `exit` but never close their own end of the connection
// afterward. The server must hang up on its own rather than waiting forever
// for more input that will never arrive.
#[test]
fn test_lsp_exits_promptly_after_exit_without_client_close() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    // Sends `shutdown`/`exit` but deliberately leaves our end of the socket
    // open, unlike the client's normal teardown on `Drop`.
    lsp.shutdown();

    lsp.expect_server_closes_connection(Duration::from_secs(5));
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
