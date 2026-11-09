//
// lsp_completions.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

// The lsp files in ark_test are for only integration tests with the Jupyter
// kernel, i.e. LSP features that require dynamic access to the R session.

use std::time::Duration;

use ark_test::DummyArkFrontend;
use serde_json::json;
use serde_json::Value;
use tower_lsp_server::ls_types as lsp_types;

/// Helper to get completion position at end of text (before trailing newline)
fn end_position(text: &str) -> u32 {
    text.trim_end_matches('\n').len() as u32
}

/// Without fetched sources, an export has no known function kind but still
/// resolves to its help topic. Disable fetching to avoid network requests.
#[test]
fn test_lsp_completions_namespace_object_resolves_to_topic() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp_with_settings(&[("oak.sourceFetching.enabled", json!(false))]);

    let text = "utils::\n";
    let uri = lsp.open_document("namespace_object.R", text);

    let items = lsp.completions(&uri, 0, end_position(text));
    let Some(item) = items.into_iter().find(|item| item.label == "osVersion") else {
        panic!("Expected `osVersion` after `utils::`");
    };

    let resolved = lsp.send_request(
        "completionItem/resolve",
        serde_json::to_value(item).unwrap(),
    );
    let documentation = resolved["documentation"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(documentation.contains("Collect Information About the Current R Session"));
}

/// `$` completions should find objects defined in the global env.
#[test]
fn test_lsp_completions_global_dollar() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    frontend.execute_request_invisibly("my_df <- data.frame(col_x = 1, col_y = 2)");

    let text = "my_df$\n";
    let uri = lsp.open_document("global_dollar.R", text);
    let items = lsp.completions(&uri, 0, end_position(text));

    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, vec!["col_x", "col_y"]);

    lsp.close_document(&uri);
    frontend.execute_request_invisibly("remove(my_df)");
}

/// Search-path completions should include variables defined in the global env.
#[test]
fn test_lsp_completions_global_search_path() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    frontend.execute_request_invisibly("my_global_var_xyz <- 42");

    // Search-path completions return fuzzy matches from the entire search path,
    // so we can only check that our variable appears somewhere in the results.
    let text = "my_global_var_x\n";
    let uri = lsp.open_document("global_search.R", text);
    let items = lsp.completions(&uri, 0, end_position(text));

    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"my_global_var_xyz"));

    lsp.close_document(&uri);
    frontend.execute_request_invisibly("remove(my_global_var_xyz)");
}

/// Argument completions should find user-defined functions in the global env.
#[test]
fn test_lsp_completions_global_arguments() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    frontend.execute_request_invisibly("my_fun <- function(aaa, bbb, ccc) NULL");

    let text = "my_fun(\n";
    let uri = lsp.open_document("global_args.R", text);
    let items = lsp.completions(&uri, 0, end_position(text));

    let mut labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    labels.sort();
    assert_eq!(labels, vec!["aaa = ", "bbb = ", "ccc = "]);

    lsp.close_document(&uri);
    frontend.execute_request_invisibly("remove(my_fun)");
}

/// Pipe completions should find column names of a global data frame.
#[test]
fn test_lsp_completions_global_pipe() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    frontend.execute_request_invisibly("my_pipe_df <- data.frame(alpha = 1, beta = 2)");

    // Completions inside a piped call include both pipe-root columns and
    // search-path symbols, so we check that the columns appear.
    let text = "my_pipe_df |> subset(\n";
    let uri = lsp.open_document("global_pipe.R", text);
    let items = lsp.completions(&uri, 0, end_position(text));

    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"alpha"));
    assert!(labels.contains(&"beta"));

    lsp.close_document(&uri);
    frontend.execute_request_invisibly("remove(my_pipe_df)");
}

/// `$` completions should find local objects during debugging.
#[test]
fn test_lsp_completions_debug_dollar() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
f <- function() {
  my_df <- data.frame(col_a = 1, col_b = 2)
  browser()
  my_df
}
f()
",
    );
    dap.recv_stopped();

    // While stopped in the debugger, request `$` completions for the local object
    let text = "my_df$\n";
    let uri = lsp.open_document("debug_dollar.R", text);
    let items = lsp.completions(&uri, 0, end_position(text));

    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, vec!["col_a", "col_b"]);

    lsp.close_document(&uri);
    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Search-path completions should include local variables during debugging.
#[test]
fn test_lsp_completions_debug_search_path() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
f <- function() {
  my_local_var_xyz <- 42
  browser()
  my_local_var_xyz
}
f()
",
    );
    dap.recv_stopped();

    // Search-path completions return fuzzy matches, so we can only check
    // that our local variable appears somewhere in the results.
    let text = "my_local_var_x\n";
    let uri = lsp.open_document("debug_search.R", text);
    let items = lsp.completions(&uri, 0, end_position(text));

    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"my_local_var_xyz"));

    lsp.close_document(&uri);
    frontend.debug_send_quit();
    dap.recv_continued();
}

/// When a different frame is selected in the debugger, `$` completions should
/// find objects from that frame's environment.
#[test]
fn test_lsp_completions_selected_frame_dollar() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
outer <- function() {
  outer_df <- data.frame(outer_col = 1)
  inner()
}
inner <- function() {
  inner_df <- data.frame(inner_col = 2)
  browser()
}
outer()
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let outer_frame_id = stack[1].id;

    // Initially, completions should find inner_df (current frame)
    let text = "inner_df$\n";
    let uri = lsp.open_document("selected_dollar_inner.R", text);
    let items = lsp.completions(&uri, 0, end_position(text));
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, vec!["inner_col"]);
    lsp.close_document(&uri);

    // Select the outer frame
    dap.evaluate(".positron_selected_frame", Some(outer_frame_id));

    // Now completions should find outer_df from the selected frame
    let text = "outer_df$\n";
    let uri = lsp.open_document("selected_dollar_outer.R", text);
    let items = lsp.completions(&uri, 0, end_position(text));
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, vec!["outer_col"]);
    lsp.close_document(&uri);

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// When a different frame is selected, search-path completions should include
/// variables from that frame's environment.
#[test]
fn test_lsp_completions_selected_frame_search_path() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
outer <- function() {
  outer_unique_var_abc <- 100
  inner()
}
inner <- function() {
  inner_unique_var_xyz <- 200
  browser()
}
outer()
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let outer_frame_id = stack[1].id;

    // Initially, completions should find inner_unique_var_xyz
    let text = "inner_unique_var_x\n";
    let uri = lsp.open_document("selected_search_inner.R", text);
    let items = lsp.completions(&uri, 0, end_position(text));
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"inner_unique_var_xyz"));
    lsp.close_document(&uri);

    // Select the outer frame
    dap.evaluate(".positron_selected_frame", Some(outer_frame_id));

    // Now completions should find outer_unique_var_abc from the selected frame
    let text = "outer_unique_var_a\n";
    let uri = lsp.open_document("selected_search_outer.R", text);
    let items = lsp.completions(&uri, 0, end_position(text));
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"outer_unique_var_abc"));
    lsp.close_document(&uri);

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// When a different frame is selected, argument completions should find
/// functions from that frame's environment.
#[test]
fn test_lsp_completions_selected_frame_arguments() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
outer <- function() {
  outer_fun <- function(outer_arg1, outer_arg2) NULL
  inner()
}
inner <- function() {
  inner_fun <- function(inner_arg) NULL
  browser()
}
outer()
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let outer_frame_id = stack[1].id;

    // Initially, completions should find inner_fun's arguments
    let text = "inner_fun(\n";
    let uri = lsp.open_document("selected_args_inner.R", text);
    let items = lsp.completions(&uri, 0, end_position(text));
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, vec!["inner_arg = "]);
    lsp.close_document(&uri);

    // Select the outer frame
    dap.evaluate(".positron_selected_frame", Some(outer_frame_id));

    // Now completions should find outer_fun's arguments
    let text = "outer_fun(\n";
    let uri = lsp.open_document("selected_args_outer.R", text);
    let items = lsp.completions(&uri, 0, end_position(text));
    let mut labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    labels.sort();
    assert_eq!(labels, vec!["outer_arg1 = ", "outer_arg2 = "]);
    lsp.close_document(&uri);

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// A custom callback's items replace the whole completion list.
#[test]
fn test_lsp_completions_custom_callback() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    frontend.execute_request_invisibly("f <- function(arg) NULL");
    frontend.execute_request_invisibly(
        ".ps.completions.registerCustomCompletionHandler('pkg', 'f', 'arg', \
         function(position) .ps.completions.createCustomCompletions(c('one', 'two')))",
    );

    let text = "f(arg)\n";
    let uri = lsp.open_document("custom_callback.R", text);
    let items = lsp.completions(&uri, 0, 5);

    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, vec!["one", "two"]);

    // With R idle the answer is complete, so the editor filters it as the
    // user types instead of requesting again on every keystroke
    let result = lsp.send_request("textDocument/completion", position_params(&uri, 0, 5));
    assert!(result.is_array());
}

/// A custom callback that raises an R error declines, leaving the call's
/// argument completions.
#[test]
fn test_lsp_completions_custom_callback_error() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    lsp.allow_log_message("Session completion source `custom` failed");

    frontend.execute_request_invisibly("f <- function(arg) NULL");
    frontend.execute_request_invisibly(
        ".ps.completions.registerCustomCompletionHandler('pkg', 'f', 'arg', \
         function(position) stop('boom'))",
    );

    let text = "f(arg)\n";
    let uri = lsp.open_document("custom_callback_error.R", text);
    let items = lsp.completions(&uri, 0, 5);

    assert_eq!(items[0].label, "arg = ");
}

/// When R is idle, completion resolve adds documentation. When R is busy,
/// `test_lsp_completion_resolve_busy()` expects the item unchanged.
#[test]
fn test_lsp_completion_resolve_idle() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let item = json!({
        "label": "paste",
        "data": { "Function": { "name": "paste", "package": "base" } },
    });
    let resolved = lsp.send_request("completionItem/resolve", item);

    let documentation = resolved["documentation"]["value"]
        .as_str()
        .unwrap_or_default();
    assert!(documentation.contains("Concatenate Strings"));
}

/// An R error that escapes the session pass fails the request without
/// blocking later document changes. A leaked Salsa handle would block the
/// database write needed to apply the change.
#[test]
fn test_lsp_completions_escaping_r_error_does_not_park_writes() -> anyhow::Result<()> {
    ark::panic::install();

    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    lsp.allow_log_message("Panic while handling request");

    let text = ".ark_test_completions_r_error$\n";
    let uri = lsp.open_document("escaping_r_error.R", text);

    let message = lsp.send_request_expect_error(
        "textDocument/completion",
        position_params(&uri, 0, end_position(text)),
    );
    assert!(message.contains("Test R error in the session completion pass"));

    lsp.send_notification(
        "textDocument/didChange",
        json!({
            "textDocument": { "uri": uri, "version": 1 },
            "contentChanges": [{ "text": "fo_changed <- 1\nfo\n" }],
        }),
    );

    let completion_id =
        lsp.send_request_raw("textDocument/completion", position_params(&uri, 1, 2));
    let symbols_id = lsp.send_request_raw(
        "textDocument/documentSymbol",
        json!({ "textDocument": { "uri": uri } }),
    );
    let responses = lsp.recv_responses(&[completion_id, symbols_id], Duration::from_secs(10))?;

    let labels: Vec<&str> = responses[0]
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| item["label"].as_str().unwrap_or_default())
        .collect();
    assert!(labels.contains(&"fo_changed"));
    assert_eq!(responses[1][0]["name"], "fo_changed");

    Ok(())
}

fn position_params(uri: &lsp_types::Uri, line: u32, character: u32) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character },
    })
}
