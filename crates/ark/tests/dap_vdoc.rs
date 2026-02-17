//
// dap_vdoc.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

// Tests for virtual document function name prefix in the debugger.
// When stepping into a function without source references, the virtual document
// content should be prefixed with `name <- ` when the frame call has a simple
// symbol in function position (e.g., `foo <- function(x) { ... }`).

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

/// Basic test: step into a function without srcrefs, verify the frame name
/// and that stepping produces correct (non-zero) line positions.
///
/// The virtual document content with the prefix is:
/// ```text
/// foo <- function(x)
/// {
///     x + 1
///     x + 2
/// }
/// ```
///
/// After stepping to `x + 1`, the position should be line 3.
/// After stepping to `x + 2`, the position should be line 4.
#[test]
fn test_dap_vdoc_fn_name_stepping() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    frontend.debug_enter_debugonce(
        "foo <- eval(parse(text = 'function(x) {\n  x + 1\n  x + 2\n}', keep.source = FALSE))\n\
         debugonce(foo)\n\
         foo(1)",
    );

    dap.recv_continued();
    dap.recv_stopped();

    // Initial stop is at the `{ body }` which can't be matched, so position is 0
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "foo()");
    assert_eq!(stack[0].line, 0);

    // Step to `x + 1`
    frontend.debug_send_vdoc_step_command("n");
    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "foo()");
    assert_eq!(stack[0].line, 3);
    assert_eq!(stack[0].column, 5);
    assert_eq!(stack[0].end_line, Some(3));
    assert_eq!(stack[0].end_column, Some(10));

    // Step to `x + 2`
    frontend.debug_send_vdoc_step_command("n");
    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "foo()");
    assert_eq!(stack[0].line, 4);
    assert_eq!(stack[0].column, 5);
    assert_eq!(stack[0].end_line, Some(4));
    assert_eq!(stack[0].end_column, Some(10));

    // Step out: function returns, back to outer browser
    frontend.debug_send_vdoc_step_out("n");
    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 1);

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Verify that the frame's source path is an `ark:` URI ending with the
/// expected source name.
#[test]
fn test_dap_vdoc_fn_name_source_path() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    frontend.debug_enter_debugonce(
        "foo <- eval(parse(text = 'function(x) {\n  x + 1\n  x + 2\n}', keep.source = FALSE))\n\
         debugonce(foo)\n\
         foo(1)",
    );

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let source = stack[0].source.as_ref().unwrap();
    let path = source.path.as_ref().unwrap();
    let name = source.name.as_ref().unwrap();

    assert_eq!(stack[0].name, "foo()");
    assert_eq!(name, "foo().R");
    assert!(path.starts_with("ark:"));
    assert!(path.ends_with("foo().R"));

    // Clean up: step out and quit
    // Use `f` (finish) to exit the function quickly
    frontend.send_execute_request("f", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();
    frontend.recv_iopub_execute_result();
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Two nested functions without srcrefs. Both frames should have correct
/// names and the inner function should have correct stepping positions.
#[test]
fn test_dap_vdoc_fn_name_nested() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    // Define two functions without srcrefs
    frontend.debug_enter_debugonce(
        "bar <- eval(parse(text = 'function() {\n  1\n}', keep.source = FALSE))\n\
         baz <- eval(parse(text = 'function() {\n  bar()\n  2\n}', keep.source = FALSE))\n\
         debugonce(baz)\n\
         baz()",
    );

    dap.recv_continued();
    dap.recv_stopped();

    // At the `{ body }` of baz
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "baz()");

    // Step to `bar()`
    frontend.debug_send_vdoc_step_command("n");
    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "baz()");
    assert_eq!(stack[0].line, 3);

    // Step into bar() with `s`
    frontend.debug_enter_debugonce("s");

    dap.recv_continued();
    dap.recv_stopped();

    // Now we should see two vdoc frames: bar on top, baz below
    let stack = dap.stack_trace();
    assert!(stack.len() >= 3);
    assert_eq!(stack[0].name, "bar()");
    assert_eq!(stack[1].name, "baz()");

    // Both should have ark: source paths with the right names
    let bar_source = stack[0].source.as_ref().unwrap();
    let baz_source = stack[1].source.as_ref().unwrap();
    assert!(bar_source.path.as_ref().unwrap().ends_with("bar().R"));
    assert!(baz_source.path.as_ref().unwrap().ends_with("baz().R"));

    // Finish bar and return to baz
    frontend.send_execute_request("f", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
    dap.recv_stopped();

    // Should be back in baz
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "baz()");

    // Finish baz, back to outer browser
    frontend.debug_send_vdoc_step_out("n");
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Namespace-qualified call (`base::identity`) should get a prefix using just
/// the function name (`identity`), not the full qualified path.
#[test]
fn test_dap_vdoc_fn_name_prefix_namespace() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    // Use base::identity with namespace qualifier
    frontend.debug_enter_debugonce("debugonce(base::identity); base::identity(42)");

    dap.recv_continued();
    dap.recv_stopped();

    // The frame_call is `base::identity(42)`:
    // - frame name (from `as_label`) keeps the namespace prefix
    // - but `call_fn_name` extracts just `identity` for the vdoc prefix
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "base::identity()");

    // `identity` is `function(x) x` (single expression, no `{`), so position
    // stays at 0 because there are no extractable source references from a
    // non-braced body.
    assert_eq!(stack[0].line, 0);

    // Clean up
    frontend.debug_send_vdoc_step_out("f");

    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Simple symbol call (`identity`) DOES get a prefix. Verify frame name and
/// that stepping works.
#[test]
fn test_dap_vdoc_fn_name_prefix_simple_symbol() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    frontend.debug_enter_debugonce("debugonce(identity); identity(42)");

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "identity()");

    // `identity` is `function(x) x` (single expression, no `{`), so position
    // stays at 0 because there are no extractable source references from a
    // non-braced body.
    assert_eq!(stack[0].line, 0);

    // Step finishes the function
    frontend.debug_send_vdoc_step_out("n");

    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Verify that file-backed functions (with srcrefs from `source()`) do NOT get
/// the `name <- ` prefix. The srcref path should return early in `frame_info`.
#[test]
fn test_dap_vdoc_fn_name_no_prefix_sourced_file() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = frontend.send_source(
        "
my_fn <- function(x) {
  browser()
  x + 1
}
my_fn(1)
",
    );
    dap.recv_stopped();

    // The frame should reference the actual file, not a virtual document
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "my_fn()");

    let source = stack[0].source.as_ref().unwrap();
    let path = source.path.as_ref().unwrap();
    // Should be a file path, not an ark: URI
    assert!(
        path.contains(&file.filename),
        "Expected file path containing {}, got {path}",
        file.filename,
    );
    assert!(!path.starts_with("ark:"));

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Non-syntactic function name should be backtick-quoted in the prefix.
/// E.g., `\`my fun\` <- function(...)`
#[test]
fn test_dap_vdoc_fn_name_prefix_backtick() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    // Define a function with a non-syntactic name (contains a space)
    frontend.debug_enter_debugonce(
        "`my fun` <- eval(parse(text = 'function(x) {\n  x + 1\n  x + 2\n}', keep.source = FALSE))\n\
         debugonce(`my fun`)\n\
         `my fun`(1)",
    );

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    // as_label produces the backtick-quoted name
    assert_eq!(stack[0].name, "`my fun`()");

    // Step to `x + 1`, verify position is correct (prefix is `\`my fun\` <- `)
    frontend.debug_send_vdoc_step_command("n");
    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "`my fun`()");
    // The body should still be on line 3 regardless of the prefix length
    assert_eq!(stack[0].line, 3);

    // Clean up
    frontend.debug_send_vdoc_step_out("f");

    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Verify stepping works correctly for a function with many statements,
/// ensuring that the reparsed source references remain accurate through
/// the entire function body.
#[test]
fn test_dap_vdoc_fn_name_full_stepping() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    // Define a 4-statement function
    frontend.debug_enter_debugonce(
        "work <- eval(parse(text = 'function(x) {\n  a <- x + 1\n  b <- a + 2\n  c <- b + 3\n  c\n}', keep.source = FALSE))\n\
         debugonce(work)\n\
         work(10)",
    );

    dap.recv_continued();
    dap.recv_stopped();

    // Initial: at `{ body }`, position 0
    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "work()");
    assert_eq!(stack[0].line, 0);

    // Step 1: `a <- x + 1` (line 3)
    frontend.debug_send_vdoc_step_command("n");
    dap.recv_continued();
    dap.recv_stopped();
    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 3);

    // Step 2: `b <- a + 2` (line 4)
    frontend.debug_send_vdoc_step_command("n");
    dap.recv_continued();
    dap.recv_stopped();
    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 4);

    // Step 3: `c <- b + 3` (line 5)
    frontend.debug_send_vdoc_step_command("n");
    dap.recv_continued();
    dap.recv_stopped();
    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 5);

    // Step 4: `c` (line 6)
    frontend.debug_send_vdoc_step_command("n");
    dap.recv_continued();
    dap.recv_stopped();
    let stack = dap.stack_trace();
    assert_eq!(stack[0].line, 6);

    // Step 5: function returns, back to outer browser
    frontend.debug_send_vdoc_step_out("n");
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Verify the `call_fn_name` logic (is the call's function position a simple
/// symbol?) through R evaluation, exercising the same `is.symbol` / `deparse`
/// logic the helper uses.
#[test]
fn test_dap_vdoc_call_fn_name_r_logic() {
    let frontend = DummyArkFrontend::lock();

    // Simple symbol: `foo` in `foo(1)` is a symbol
    frontend.send_execute_request(
        "is.symbol(quote(foo(1))[[1]])",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] TRUE");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Namespace-qualified: `base::identity` is a `::` call, not a symbol
    frontend.send_execute_request(
        "is.symbol(quote(base::identity(1))[[1]])",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] FALSE");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // But the RHS of `::` IS a symbol that we extract
    frontend.send_execute_request(
        "is.symbol(quote(base::identity(1))[[1]][[3]])",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] TRUE");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Non-syntactic name: is a symbol, deparse with `backtick = TRUE` quotes it
    frontend.send_execute_request(
        "deparse(quote(`my fun`(1))[[1]], backtick = TRUE)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] \"`my fun`\"");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Anonymous function: `(function(x) x)` is a call, not a symbol
    frontend.send_execute_request(
        "is.symbol(quote((function(x) x)(1))[[1]])",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] FALSE");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Verify that repeated debug sessions with the same function produce
/// consistent behavior (no stale virtual documents).
#[test]
fn test_dap_vdoc_fn_name_repeated_debug() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    // Define function (inside browser, so it cycles stop_debug/start_debug)
    frontend.debug_send_vdoc_step_command(
        "rpt <- eval(parse(text = 'function() {\n  1\n  2\n}', keep.source = FALSE))",
    );

    dap.recv_continued();
    dap.recv_stopped();

    // First debug session
    frontend.debug_enter_debugonce("debugonce(rpt); rpt()");

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "rpt()");

    // Finish the function
    frontend.debug_send_vdoc_step_out("f");

    dap.recv_continued();
    dap.recv_stopped();

    // Second debug session with the same function
    frontend.debug_enter_debugonce("debugonce(rpt); rpt()");

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "rpt()");

    // Step to `1`, verify stepping still works in the second session
    frontend.debug_send_vdoc_step_command("n");
    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack[0].name, "rpt()");

    // Finish and clean up
    frontend.debug_send_vdoc_step_out("f");

    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Test that virtual document content contains the prefix assignment.
#[test]
fn test_dap_vdoc_content_has_prefix() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    frontend.debug_enter_debugonce(
        "foo <- eval(parse(text = 'function(x) {\n  x + 1\n}', keep.source = FALSE))\n\
         debugonce(foo)\n\
         foo(1)",
    );

    dap.recv_continued();
    dap.recv_stopped();

    // Get the source path from the stack frame
    let stack = dap.stack_trace();
    let source = stack[0].source.as_ref().unwrap();
    let path = source.path.as_ref().unwrap();

    // Query the virtual document content
    // This executes code in debug mode, generating DAP continued/stopped events
    let content = frontend.get_virtual_document(path);
    dap.recv_continued();
    dap.recv_stopped();
    let content = content.expect("Virtual document should exist");

    // Verify the content starts with the prefix assignment
    assert!(
        content.starts_with("foo <- function"),
        "Expected vdoc to start with 'foo <- function', got: {}",
        content.lines().next().unwrap_or("")
    );

    // Clean up
    frontend.debug_send_vdoc_step_out("f");
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Test vdoc content for namespace-qualified call uses just the function name.
#[test]
fn test_dap_vdoc_content_namespace_prefix() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    frontend.debug_enter_debugonce("debugonce(base::identity); base::identity(42)");

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let source = stack[0].source.as_ref().unwrap();
    let path = source.path.as_ref().unwrap();

    let content = frontend.get_virtual_document(path);
    dap.recv_continued();
    dap.recv_stopped();
    let content = content.expect("Virtual document should exist");

    // For namespace-qualified calls, the prefix should use just the function name
    assert!(
        content.starts_with("identity <- function"),
        "Expected vdoc to start with 'identity <- function', got: {}",
        content.lines().next().unwrap_or("")
    );

    // Clean up
    frontend.debug_send_vdoc_step_out("f");
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Test vdoc content for non-syntactic function name uses backticks.
#[test]
fn test_dap_vdoc_content_backtick_prefix() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    frontend.debug_enter_debugonce(
        "`my fun` <- eval(parse(text = 'function(x) {\n  x + 1\n}', keep.source = FALSE))\n\
         debugonce(`my fun`)\n\
         `my fun`(1)",
    );

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let source = stack[0].source.as_ref().unwrap();
    let path = source.path.as_ref().unwrap();

    let content = frontend.get_virtual_document(path);
    dap.recv_continued();
    dap.recv_stopped();
    let content = content.expect("Virtual document should exist");

    // Non-syntactic names should be backtick-quoted in the prefix
    assert!(
        content.starts_with("`my fun` <- function"),
        "Expected vdoc to start with '`my fun` <- function', got: {}",
        content.lines().next().unwrap_or("")
    );

    // Clean up
    frontend.debug_send_vdoc_step_out("f");
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Test vdoc content for `pkg::fun` style call uses just the function name.
#[test]
fn test_dap_vdoc_content_double_colon_prefix() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    // Use a real namespace function (tools::md5sum exists in base R)
    frontend.debug_enter_debugonce("debugonce(tools::md5sum); tools::md5sum(character())");

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let source = stack[0].source.as_ref().unwrap();
    let path = source.path.as_ref().unwrap();

    let content = frontend.get_virtual_document(path);
    dap.recv_continued();
    dap.recv_stopped();
    let content = content.expect("Virtual document should exist");

    // For `pkg::fun` calls, the prefix should use just the function name
    assert!(
        content.starts_with("md5sum <- function"),
        "Expected vdoc to start with 'md5sum <- function', got: {}",
        content.lines().next().unwrap_or("")
    );

    // Clean up
    frontend.debug_send_vdoc_step_out("f");
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Test vdoc content for `pkg:::fun` style call uses just the function name.
#[test]
fn test_dap_vdoc_content_triple_colon_prefix() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    // Use an internal namespace function (tools:::httpdPort exists in base R)
    frontend.debug_enter_debugonce("debugonce(tools:::httpdPort); tools:::httpdPort()");

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let source = stack[0].source.as_ref().unwrap();
    let path = source.path.as_ref().unwrap();

    let content = frontend.get_virtual_document(path);
    dap.recv_continued();
    dap.recv_stopped();
    let content = content.expect("Virtual document should exist");

    // For `pkg:::fun` calls, the prefix should use just the function name
    assert!(
        content.starts_with("httpdPort <- function"),
        "Expected vdoc to start with 'httpdPort <- function', got: {}",
        content.lines().next().unwrap_or("")
    );

    // Clean up
    frontend.debug_send_vdoc_step_out("f");
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Test vdoc content for `obj$method` style call has no prefix.
#[test]
fn test_dap_vdoc_content_dollar_no_prefix() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();
    dap.recv_stopped();

    // Create an object with a method
    frontend.debug_enter_debugonce(
        "obj <- list()\n\
         obj$method <- eval(parse(text = 'function(x) {\n  x * 2\n}', keep.source = FALSE))\n\
         debugonce(obj$method)\n\
         obj$method(5)",
    );

    dap.recv_continued();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let source = stack[0].source.as_ref().unwrap();
    let path = source.path.as_ref().unwrap();

    let content = frontend.get_virtual_document(path);
    dap.recv_continued();
    dap.recv_stopped();
    let content = content.expect("Virtual document should exist");

    // For `obj$method` calls, there should be no prefix (starts directly with function)
    assert!(
        content.starts_with("function"),
        "Expected vdoc to start with 'function' (no prefix), got: {}",
        content.lines().next().unwrap_or("")
    );

    // Clean up
    frontend.debug_send_vdoc_step_out("f");
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}
