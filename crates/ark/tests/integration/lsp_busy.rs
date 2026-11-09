//
// lsp_busy.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

// Hover needs R. When R is busy, it should answer `null` promptly instead of
// blocking the LSP main loop until R is back at a prompt. Signature help
// answers from the callee's static description without parameter
// documentation, completions answer with their static items, and completion
// resolve returns the item unchanged.

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

// Oak can't describe a function defined only in the console, so the session
// supplies the signature.
#[test]
fn test_lsp_signature_help_idle_console_function() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    frontend.execute_request_invisibly("ark_console_fn <- function(alpha, beta = 2) NULL");

    let uri = lsp.open_document("signature_help_console.R", "ark_console_fn(1, )\n");
    let result = lsp.send_request("textDocument/signatureHelp", position_params(&uri, 0, 18));

    assert_eq!(
        result["signatures"][0]["label"],
        "ark_console_fn(alpha, beta = 2)"
    );
    assert_eq!(result["activeParameter"], 1);

    frontend.execute_request_invisibly("rm(ark_console_fn)");
}

// With R idle, the console definition wins over the document's, as it does
// for call argument completions.
#[test]
fn test_lsp_signature_help_idle_redefined_in_console() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    frontend.execute_request_invisibly("ark_redef_fn <- function(x, y = 'session') NULL");

    let uri = lsp.open_document(
        "signature_help_redefined.R",
        "ark_redef_fn <- function(a) NULL\nark_redef_fn(\n",
    );
    let result = lsp.send_request("textDocument/signatureHelp", position_params(&uri, 1, 13));

    assert_eq!(
        result["signatures"][0]["label"],
        "ark_redef_fn(x, y = \"session\")"
    );

    frontend.execute_request_invisibly("rm(ark_redef_fn)");
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

// While R is busy, the label and active parameter come from the callee's
// static description, without parameter documentation.
#[test]
fn test_lsp_signature_help_busy() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document(
        "signature_help_busy.R",
        "ark_busy_fn <- function(alpha, beta = 'b') NULL\nark_busy_fn(1, )\n",
    );

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let id = lsp.send_request_raw("textDocument/signatureHelp", position_params(&uri, 1, 15));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    let signature = &responses[0]["signatures"][0];
    assert_eq!(signature["label"], "ark_busy_fn(alpha, beta = 'b')");
    assert_eq!(responses[0]["activeParameter"], 1);
    assert_eq!(signature["parameters"][1]["documentation"], Value::Null);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

// Without a static description or an idle R session, no signature is available.
#[test]
fn test_lsp_signature_help_busy_undescribed() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("signature_help_busy_undescribed.R", "ark_unknown_fn(\n");

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let id = lsp.send_request_raw("textDocument/signatureHelp", position_params(&uri, 0, 15));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    assert_eq!(responses, vec![Value::Null]);

    frontend.recv_iopub_idle();
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

    let id = lsp.send_request_raw("textDocument/hover", position_params(&uri, 0, 2));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    assert_eq!(responses, vec![Value::Null]);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

// A static request queued behind signature help must not wait on R either
#[test]
fn test_lsp_signature_help_busy_does_not_stall_main_loop() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document(
        "signature_help_main_loop.R",
        "foo <- function(x) NULL\nfoo(\n",
    );

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let signature_help_id =
        lsp.send_request_raw("textDocument/signatureHelp", position_params(&uri, 1, 4));
    let symbols_id = lsp.send_request_raw(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    let responses = lsp.recv_responses(&[signature_help_id, symbols_id], BUSY_TIMEOUT)?;
    assert_eq!(responses[0]["signatures"][0]["label"], "foo(x)");
    assert_eq!(responses[1][0]["name"], "foo");

    frontend.recv_iopub_idle();
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

    let hover_id = lsp.send_request_raw("textDocument/hover", position_params(&uri, 1, 2));
    let symbols_id = lsp.send_request_raw(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    let responses = lsp.recv_responses(&[hover_id, symbols_id], BUSY_TIMEOUT)?;
    assert_eq!(responses[0], Value::Null);
    assert_eq!(responses[1][0]["name"], "foo");

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

/// Labels of a completion list answered while R is busy. The list is marked
/// incomplete so the editor asks again once R may be idle.
fn busy_completion_labels(response: &Value) -> Vec<&str> {
    assert_eq!(response["isIncomplete"], true);
    let Some(items) = response["items"].as_array() else {
        panic!("Expected a completion list, got: {response}");
    };
    items
        .iter()
        .map(|item| item["label"].as_str().unwrap_or_default())
        .collect()
}

// `foo$` requires session names. When R is busy, unrelated sources must
// not fill the empty result.
#[test]
fn test_lsp_completions_busy_extractor_is_empty() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("completions_busy_extractor.R", "foo$\n");

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let id = lsp.send_request_raw("textDocument/completion", position_params(&uri, 0, 4));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    assert_eq!(busy_completion_labels(&responses[0]), Vec::<&str>::new());

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

// In `x["|"]`, `|` marks the cursor. String subsetting requires session
// names, and while R is busy the string context must not be filled with
// keywords or other composite items. That it isn't recognized as a file path
// is checked by `test_string_subset_lists_no_files_without_session()`.
#[test]
fn test_lsp_completions_busy_string_subset_is_empty() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("completions_busy_string_subset.R", "x[\"\"]\n");

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let id = lsp.send_request_raw("textDocument/completion", position_params(&uri, 0, 3));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    assert_eq!(busy_completion_labels(&responses[0]), Vec::<&str>::new());

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

// Keywords and symbols from documents in the workspace remain available
// while R is busy. Search-path symbols such as `format()` require the session.
#[test]
fn test_lsp_completions_busy_identifier() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    lsp.open_document(
        "completions_busy_workspace.R",
        "fo_workspace <- function() NULL\n",
    );
    let uri = lsp.open_document("completions_busy_identifier.R", "fo_document <- 1\nfo\n");

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let id = lsp.send_request_raw("textDocument/completion", position_params(&uri, 1, 2));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    let labels = busy_completion_labels(&responses[0]);
    assert!(labels.contains(&"for"));
    assert!(labels.contains(&"fo_document"));
    assert!(labels.contains(&"fo_workspace"));
    assert!(!labels.contains(&"format"));

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

// When R is busy, an unavailable custom callback must not suppress the
// fallback list, which starts with argument names from the callee's static
// description.
#[test]
fn test_lsp_completions_busy_call_arguments() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document(
        "completions_busy_call_arguments.R",
        "f <- function(arg_one, arg_two) NULL\nf(arg)\n",
    );

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let id = lsp.send_request_raw("textDocument/completion", position_params(&uri, 1, 5));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    let labels = busy_completion_labels(&responses[0]);
    assert_eq!(labels[0..2], ["arg_one = ", "arg_two = "]);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

// The roxygen tag answer comes from static sources only, so it is complete
// even though R is busy.
#[test]
fn test_lsp_completions_busy_static_answer_is_complete() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("completions_busy_static.R", "#' @\n");

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let id = lsp.send_request_raw("textDocument/completion", position_params(&uri, 0, 4));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    // Either a plain array or `null`, never an incomplete list
    assert_eq!(responses[0]["isIncomplete"], Value::Null);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

// Namespace completions need no R session. Disable source fetching to keep
// the answer complete and avoid downloading the base R archive for `utils`.
#[test]
fn test_lsp_completions_busy_namespace_is_complete() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp_with_settings(&[("oak.sourceFetching.enabled", json!(false))]);

    let uri = lsp.open_document("completions_busy_namespace.R", "utils::\n");

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let id = lsp.send_request_raw("textDocument/completion", position_params(&uri, 0, 7));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    let Some(items) = responses[0].as_array() else {
        panic!("Expected a complete answer, got: {}", responses[0]);
    };
    let labels: Vec<&str> = items
        .iter()
        .map(|item| item["label"].as_str().unwrap_or_default())
        .collect();
    assert!(labels.contains(&"adist"));
    assert!(!labels.contains(&"as.bibentry.bibentry"));

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

// File path completions return a complete answer while R is busy. Without
// an enclosing environment directory, relative paths use the workspace root.
#[test]
fn test_lsp_completions_busy_file_paths_are_complete() -> anyhow::Result<()> {
    let workspace = tempfile::tempdir()?;
    std::fs::create_dir_all(workspace.path().join("R/sub"))?;
    std::fs::write(workspace.path().join("R/utils.R"), "")?;

    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp_in_workspace(workspace.path());

    let uri = lsp.open_document_at_path(&workspace.path().join("script.R"), "source(\"R/\")\n");

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let id = lsp.send_request_raw("textDocument/completion", position_params(&uri, 0, 10));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    let Some(items) = responses[0].as_array() else {
        panic!("Expected a complete answer, got: {}", responses[0]);
    };
    let labels: Vec<&str> = items
        .iter()
        .map(|item| item["label"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(labels, vec!["sub/", "utils.R"]);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

// Lazy data names must remain available while the kernel's R session is busy.
// Source fetching is disabled, so an incomplete answer means the startup
// sidecar job is still pending. Wait for it before making R busy.
#[test]
fn test_lsp_completions_busy_namespace_lists_lazy_data() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp_with_settings(&[("oak.sourceFetching.enabled", json!(false))]);

    let uri = lsp.open_document("completions_busy_lazy_data.R", "datasets::\n");

    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let id = lsp.send_request_raw("textDocument/completion", position_params(&uri, 0, 10));
        let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
        if responses[0].is_array() {
            break;
        }
        assert_eq!(responses[0]["isIncomplete"], true);
        if std::time::Instant::now() > deadline {
            panic!("`datasets::` is still incomplete: {}", responses[0]);
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let id = lsp.send_request_raw("textDocument/completion", position_params(&uri, 0, 10));
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    let Some(items) = responses[0].as_array() else {
        panic!("Expected a complete answer, got: {}", responses[0]);
    };
    let labels: Vec<&str> = items
        .iter()
        .map(|item| item["label"].as_str().unwrap_or_default())
        .collect();
    assert!(labels.contains(&"mtcars"));
    assert!(labels.contains(&"BJsales.lead"));

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

// A document-symbol request queued behind a completion must not wait for R.
#[test]
fn test_lsp_completions_busy_does_not_stall_main_loop() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let uri = lsp.open_document("completions_main_loop.R", "foo <- 1\nfo\n");

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let completion_id =
        lsp.send_request_raw("textDocument/completion", position_params(&uri, 1, 2));
    let symbols_id = lsp.send_request_raw(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    let responses = lsp.recv_responses(&[completion_id, symbols_id], BUSY_TIMEOUT)?;
    assert!(busy_completion_labels(&responses[0]).contains(&"foo"));
    assert_eq!(responses[1][0]["name"], "foo");

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

#[test]
fn test_lsp_completion_resolve_busy() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let item = json!({
        "label": "paste",
        "data": { "Function": { "name": "paste", "package": "base" } },
    });

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let id = lsp.send_request_raw("completionItem/resolve", item.clone());
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    assert_eq!(responses, vec![item]);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}

#[test]
fn test_lsp_completion_resolve_static_item_busy() -> anyhow::Result<()> {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let item = json!({
        "label": "for",
        "data": { "Keyword": { "name": "for" } },
    });

    frontend.send_execute_request(BUSY_CODE, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let id = lsp.send_request_raw("completionItem/resolve", item.clone());
    let responses = lsp.recv_responses(&[id], BUSY_TIMEOUT)?;
    assert_eq!(responses, vec![item]);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    Ok(())
}
