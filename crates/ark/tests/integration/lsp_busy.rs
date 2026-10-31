//
// lsp_busy.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

// Hover and signature help need R. When R is busy, they should answer `null`
// promptly instead of blocking the LSP main loop until R is back at a prompt.

use std::time::Duration;

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;
use serde_json::json;
use serde_json::Value;
use tower_lsp_server::ls_types as lsp_types;

// Must outlast `BUSY_TIMEOUT`, so that a request blocked until R is idle
// times out instead of being answered when the sleep ends
const BUSY_CODE: &str = "Sys.sleep(4)";
const BUSY_TIMEOUT: Duration = Duration::from_secs(2);

fn position_params(uri: &lsp_types::Uri, line: u32, character: u32) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character },
    })
}

#[test]
fn test_lsp_signature_help_idle() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("signature_help_idle.R", "toupper(\n");
    let result = lsp.send_request("textDocument/signatureHelp", position_params(&uri, 0, 8));

    assert_eq!(result["signatures"][0]["label"], "toupper(x)");
}

#[test]
fn test_lsp_hover_idle() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("hover_idle.R", "toupper(x)\n");
    let result = lsp.send_request("textDocument/hover", position_params(&uri, 0, 2));

    let value = result["contents"]["value"].as_str().unwrap_or_default();
    assert!(value.contains("toupper"));
}

#[test]
fn test_lsp_signature_help_busy() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("signature_help_busy.R", "toupper(\n");

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let start = std::time::Instant::now();

    let id = lsp.send_request_raw("textDocument/signatureHelp", position_params(&uri, 0, 8));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT);
    eprintln!("DEBUG responses after {:?}: {responses:?}", start.elapsed());
    let responses = responses?;
    assert_eq!(responses, vec![Value::Null]);

    frontend.recv_iopub_idle();
    eprintln!("DEBUG idle after {:?}", start.elapsed());
    frontend.recv_shell_execute_reply();

    Ok(())
}

#[test]
fn test_lsp_hover_busy() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("hover_busy.R", "toupper(x)\n");

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let start = std::time::Instant::now();

    let id = lsp.send_request_raw("textDocument/hover", position_params(&uri, 0, 2));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT);
    eprintln!("DEBUG responses after {:?}: {responses:?}", start.elapsed());
    let responses = responses?;
    assert_eq!(responses, vec![Value::Null]);

    frontend.recv_iopub_idle();
    eprintln!("DEBUG idle after {:?}", start.elapsed());
    frontend.recv_shell_execute_reply();

    Ok(())
}

// A static request queued behind signature help must not wait on R either
#[test]
fn test_lsp_signature_help_busy_does_not_stall_main_loop() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("signature_help_main_loop.R", "foo <- 1\ntoupper(\n");

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let start = std::time::Instant::now();

    let signature_help_id =
        lsp.send_request_raw("textDocument/signatureHelp", position_params(&uri, 1, 8));
    let symbols_id = lsp.send_request_raw(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    let responses = lsp.recv_responses(&[signature_help_id, symbols_id], BUSY_TIMEOUT);
    eprintln!("DEBUG responses after {:?}: {responses:?}", start.elapsed());
    let responses = responses?;
    assert_eq!(responses[0], Value::Null);
    assert_eq!(responses[1][0]["name"], "foo");

    frontend.recv_iopub_idle();
    eprintln!("DEBUG idle after {:?}", start.elapsed());
    frontend.recv_shell_execute_reply();

    Ok(())
}

// A static request queued behind hover must not wait on R either
#[test]
fn test_lsp_hover_busy_does_not_stall_main_loop() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("hover_main_loop.R", "foo <- 1\ntoupper(x)\n");

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let start = std::time::Instant::now();

    let hover_id = lsp.send_request_raw("textDocument/hover", position_params(&uri, 1, 2));
    let symbols_id = lsp.send_request_raw(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    let responses = lsp.recv_responses(&[hover_id, symbols_id], BUSY_TIMEOUT);
    eprintln!("DEBUG responses after {:?}: {responses:?}", start.elapsed());
    let responses = responses?;
    assert_eq!(responses[0], Value::Null);
    assert_eq!(responses[1][0]["name"], "foo");

    frontend.recv_iopub_idle();
    eprintln!("DEBUG idle after {:?}", start.elapsed());
    frontend.recv_shell_execute_reply();

    Ok(())
}
