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
