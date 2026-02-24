use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

#[test]
fn test_execute_request_browser() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request("browser()", |result| {
        assert!(result.contains("Called from: top level"));
    });

    frontend.execute_request_invisibly("Q");
}

#[test]
fn test_execute_request_browser_continue() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request("browser()", |result| {
        assert!(result.contains("Called from: top level"));
    });

    frontend.execute_request_invisibly("n");
}

// Empty input in the debugger should count as `n` and advance the debugger.
// https://github.com/posit-dev/ark/issues/1006
#[test]
fn test_execute_request_browser_empty_input() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request("{browser(); 1; 2}", |result| {
        assert!(result.contains("Called from: top level"));
    });

    // Step past browser() with empty input
    let code = "";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    // Stepping produces debug output
    assert!(frontend.recv_iopub_execute_result().contains("debug at"));
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Step past `1` with empty input
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert!(frontend.recv_iopub_execute_result().contains("debug at"));
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Step past `2` with empty input - this exits the debugger and returns the result
    frontend.execute_request("", |result| {
        assert!(result.contains("[1] 2"));
    });

    // Now we should be out of the debugger. Verify by running normal code.
    frontend.execute_request("1 + 1", |result| {
        assert!(result.contains("[1] 2"));
    });
}

// When `browserNLdisabled = TRUE`, empty input should not advance the debugger.
// https://github.com/posit-dev/ark/issues/1006
#[test]
fn test_execute_request_browser_empty_input_disabled() {
    let frontend = DummyArkFrontend::lock();

    // Set the option to disable empty input advancing the debugger
    frontend.execute_request_invisibly("options(browserNLdisabled = TRUE)");

    frontend.execute_request("browser()", |result| {
        assert!(result.contains("Called from: top level"));
    });

    // Empty input should NOT advance the debugger, we should still be in the browser
    let code = "";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Verify we're still in the browser by evaluating something
    frontend.execute_request("42", |result| {
        assert!(result.contains("[1] 42"));
    });

    // Now quit the browser
    frontend.execute_request_invisibly("Q");

    // Reset the option
    frontend.execute_request_invisibly("options(browserNLdisabled = FALSE)");
}

#[test]
fn test_execute_request_browser_nested() {
    // Test nested browser() calls - entering a browser within a browser
    let frontend = DummyArkFrontend::lock();

    // Start first browser
    frontend.execute_request("browser()", |result| {
        assert!(result.contains("Called from: top level"));
    });

    // Evaluate a value in the outer browser
    frontend.execute_request("42", |result| assert!(result.contains("[1] 42")));

    // Start nested browser from within the first browser
    // Nested browser() produces execute_result output
    frontend.execute_request("browser()", |_result| {});

    // Evaluate a command in the nested browser
    frontend.execute_request("1", |result| assert!(result.contains("[1] 1")));

    // Evaluate another value in the nested browser
    frontend.execute_request("\"hello\"", |result| assert!(result.contains("hello")));

    // Throw an error in the nested browser. `globalErrorHandler` runs and
    // exits all the way to top level.
    let code = "stop('error in nested')";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    let evalue = frontend.recv_iopub_execute_error();
    assert!(evalue.contains("error in nested"));
    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );

    // Now back at top level. Start a new browser session to test
    // continue-from-nested and error-in-parent scenarios.
    frontend.execute_request("browser()", |result| {
        assert!(result.contains("Called from: top level"));
    });

    // Enter nested browser
    frontend.execute_request("browser()", |_result| {});

    // Continue to exit the nested browser and return to parent
    frontend.execute_request_invisibly("c");

    // Back in the parent browser, evaluate another value
    frontend.execute_request("3.14", |result| assert!(result.contains("[1] 3.14")));

    // Throw an error in the parent browser. `globalErrorHandler` runs and
    // exits to top level.
    let code = "stop('error in parent')";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    let evalue = frontend.recv_iopub_execute_error();
    assert!(evalue.contains("error in parent"));
    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );
}

#[test]
fn test_execute_request_browser_error() {
    // When evaluating in the debugger, our local calling error handler
    // ensures `globalErrorHandler` runs. This gives proper backtrace
    // capturing and error formatting, but exits the debugger via
    // `invokeRestart("abort")`.

    let frontend = DummyArkFrontend::lock();

    frontend.execute_request("browser()", |result| {
        assert!(result.contains("Called from: top level"));
    });

    frontend.send_execute_request("stop('foobar')", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, "stop('foobar')");

    // `globalErrorHandler` formats the error and exits to top level
    let evalue = frontend.recv_iopub_execute_error();
    assert!(evalue.contains("foobar"));
    frontend.recv_iopub_idle();

    frontend.recv_shell_execute_reply_exception();
}

#[test]
fn test_execute_request_browser_incomplete() {
    // Set RUST_BACKTRACE to ensure backtraces are captured. We used to leak
    // backtraces in syntax error messages, and this shouldn't happen even when
    // `RUST_BACKTRACE` is set.
    std::env::set_var("RUST_BACKTRACE", "1");

    let frontend = DummyArkFrontend::lock();

    frontend.execute_request("browser()", |result| {
        assert!(result.contains("Called from: top level"));
    });

    let code = "1 +";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.assert_stream_stderr_contains("Error: Can't parse incomplete input");
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

// Test that a multiline input in the browser doesn't throw off our prompt info
// detection logic https://github.com/posit-dev/positron/issues/5928
#[test]
fn test_execute_request_browser_multiline() {
    let frontend = DummyArkFrontend::lock();

    // Wrap in a function to get a frame on the stack so we aren't at top level.
    // Careful to not send any newlines after `fn()`, as that advances the debugger!
    let code = "
fn <- function() {
  browser()
}
fn()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // We aren't at top level, so this comes as an iopub stream
    frontend.assert_stream_stdout_contains("Called from: fn()");
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Execute a multiline statement while paused in the debugger
    let code = "1 +
        1";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // Also received as iopub stream because we aren't at top level, we are in the debugger
    frontend.assert_stream_stdout_contains("[1] 2");
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    frontend.execute_request_invisibly("Q");
}

#[test]
fn test_execute_request_browser_stdin() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request("browser()", |result| {
        assert!(result.contains("Called from: top level"));
    });

    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };
    let code = "readline('prompt>')";
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    let prompt = frontend.recv_stdin_input_request();
    assert_eq!(prompt, String::from("prompt>"));

    frontend.send_stdin_input_reply(String::from("hi"));

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] \"hi\"");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    frontend.execute_request_invisibly("Q");
}

#[test]
fn test_execute_request_browser_multiple_expressions() {
    let frontend = DummyArkFrontend::lock();

    // Ideally the evaluation of `1` would be cancelled
    let code = "browser()\n1";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 1");
    frontend.assert_stream_stdout_contains("Called from: top level");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Even if we could cancel pending expressions, it would still be possible
    // to run multiple expressions in a debugger prompt
    let code = "1\n2";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 2");
    frontend.assert_stream_stdout_contains("[1] 1");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // But getting in a nested browser session with a pending expression would
    // cancel it (not the case currently)
    let code = "browser()\n1";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 1");
    frontend.assert_stream_stdout_contains("Called from: top level");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Quit session
    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_execute_request_browser_local_variable() {
    let frontend = DummyArkFrontend::lock();

    let code = "local({\n  local_foo <- 1\n  browser()\n})";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.assert_stream_stdout_contains("Called from: eval(quote({");
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    let code = "local_foo";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // Should ideally be `recv_iopub_execute_result()`, but auto-printing
    // detection currently does not work reliably in debug REPLs
    frontend.assert_stream_stdout_contains("[1] 1");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    frontend.execute_request_invisibly("Q");
}

// Can debug the base environment (parent is the empty environment)
#[test]
fn test_browser_in_base_env() {
    let frontend = DummyArkFrontend::lock();

    let code = "evalq(browser(), baseenv())";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // Inside `evalq()` we aren't at top level, so this comes as an iopub stream
    // and not an execute result
    frontend.assert_stream_stdout_contains("Called from: evalq(browser(), baseenv())");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // While paused in the debugger, evaluate a simple expression
    let code = "1 + 1";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.assert_stream_stdout_contains("[1] 2");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_execute_request_browser_braced_step_out() {
    let frontend = DummyArkFrontend::lock();

    // Evaluate `{browser()}` which enters the debugger
    frontend.execute_request("{browser()}", |result| {
        assert!(result.contains("Called from: top level"));
    });

    // Step once with `n` to leave the debugger (the braced expression completes)
    frontend.execute_request_invisibly("n");

    // Now evaluate `{1}` - this should NOT trigger the debugger
    // and should return the result normally
    frontend.execute_request("{1}", |result| {
        assert!(result.contains("[1] 1"));
    });
}

// The minimal environment we can debug in: access to base via `::`. This might
// be a problem for very specialised sandboxing environment, but they can
// temporarily add `::` while debugging.
#[test]
fn test_browser_in_sandboxing_environment() {
    let frontend = DummyArkFrontend::lock();

    let code = "
env <- new.env(parent = emptyenv())
env$`::` <- `::`
evalq(base::browser(), env)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // Inside `evalq()` we aren't at top level, so this comes as an iopub stream
    // and not an execute result
    frontend.assert_stream_stdout_contains("Called from: evalq(base::browser(), env)");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // While paused in the debugger, evaluate a simple expression that only
    // requires `::`
    let code = "base::list";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.assert_stream_stdout_contains("function (...)  .Primitive(\"list\")");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}
