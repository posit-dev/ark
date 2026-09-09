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

// An abrupt client disconnect must not panic the `ark-lsp` thread.
#[test]
fn test_lsp_survives_abrupt_disconnect() {
    // Capture `ark-lsp` panics because `catch_unwind()` on this test thread
    // cannot observe them. Chain the prior hook so the panic remains in test output.
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

    // Wait for the `ark-lsp` thread to observe the reset.
    std::thread::sleep(Duration::from_millis(200));

    if let Some(message) = panic_message.lock().unwrap().take() {
        panic!("`ark-lsp` thread panicked on abrupt disconnect: {message}");
    }

    let lsp2 = frontend.start_lsp();
    assert!(lsp2.server_capabilities().completion_provider.is_some());
}

// `exit` must close the server side even if the client socket remains open.
#[test]
fn test_lsp_exits_promptly_after_exit_without_client_close() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    // `shutdown()` sends `shutdown`/`exit` without closing the client socket.
    lsp.shutdown();

    lsp.expect_server_closes_connection(Duration::from_secs(5));
}

// A panicking request handler must return an error without ending the LSP session.
#[test]
fn test_lsp_panicking_request_is_task_local() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let message = lsp.send_request_expect_error("ark/testPanic", json!({}));
    assert!(message.contains("Panic while handling request"));

    let uri = lsp.open_document("test_panic_request.R", "x <- 1\n");
    lsp.completions(&uri, 0, 0);
}

// A notification handler panic must show a crash dialog before closing the LSP connection.
#[test]
fn test_lsp_panicking_notification_ends_session() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    // Allow the expected panic log because its auxiliary loop can deliver it
    // before or after the crash dialog.
    lsp.allow_log_message("Panic while handling event");

    lsp.send_notification("ark/testPanicNotification", json!({}));

    // Read `window/showMessageRequest` first because shutdown races it.
    lsp.recv_server_request("window/showMessageRequest");
    lsp.expect_server_closes_connection(Duration::from_secs(5));

    // Skip `shutdown()` because the server has already closed the connection.
    lsp.disconnect_abruptly();
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
