//
// lsp_completions.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

// The lsp files in ark_test are for only integration tests with the Jupyter
// kernel, i.e. LSP features that require dynamic access to the R session.

use ark_test::DummyArkFrontend;

/// Helper to get completion position at end of text (before trailing newline)
fn end_position(text: &str) -> u32 {
    text.trim_end_matches('\n').len() as u32
}

#[test]
fn test_lsp_completions_basic() {
    let frontend = DummyArkFrontend::lock();
    let mut lsp = frontend.start_lsp();

    let text = "base::\n";
    let uri = lsp.open_document("test.R", text);

    let items = lsp.completions(&uri, 0, end_position(text));
    assert!(!items.is_empty());

    // Sanity check that well-known functions appear (can't be exact since base has hundreds)
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"paste"));
    assert!(labels.contains(&"print"));
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
