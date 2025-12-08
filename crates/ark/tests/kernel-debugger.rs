use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark::fixtures::DummyArkFrontend;

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

    // Throw an error in the nested browser
    let code = "stop('error in nested')";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_stream_stderr("Error: error in nested\n");
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Continue to exit the nested browser and return to parent
    frontend.execute_request_invisibly("c");

    // Back in the parent browser, evaluate another value
    frontend.execute_request("3.14", |result| assert!(result.contains("[1] 3.14")));

    // Throw an error in the outer browser
    let code = "stop('error in parent')";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_stream_stderr("Error: error in parent\n");
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    frontend.execute_request("NA", |result| assert!(result.contains("[1] NA")));
    // Quit the outer browser
    frontend.execute_request_invisibly("Q");
}

#[test]
fn test_execute_request_browser_error() {
    // The behaviour for errors is different in browsers than at top-level
    // because our global handler does not run in that case. Instead the error
    // is streamed on IOPub::Stderr and a regular execution result is sent as
    // response.

    let frontend = DummyArkFrontend::lock();

    frontend.execute_request("browser()", |result| {
        assert!(result.contains("Called from: top level"));
    });

    frontend.send_execute_request("stop('foobar')", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, "stop('foobar')");

    frontend.recv_iopub_stream_stderr("Error: foobar\n");
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    frontend.execute_request_invisibly("Q");
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

    frontend.recv_iopub_stream_stderr("Error: Can't parse incomplete input\n");
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
    frontend.recv_iopub_stream_stdout("Called from: fn()\n");
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
    frontend.recv_iopub_stream_stdout("[1] 2\n");
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

    let options = ExecuteRequestOptions { allow_stdin: true };
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

    frontend.recv_iopub_stream_stdout("Called from: top level \n");

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 1");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Even if we could cancel pending expressions, it would still be possible
    // to run multiple expressions in a debugger prompt
    let code = "1\n2";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_stream_stdout("[1] 1\n");

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 2");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // But getting in a nested browser session with a pending expression would
    // cancel it (not the case currently)
    let code = "browser()\n1";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_stream_stdout("Called from: top level \n");

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 1");
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

    frontend.recv_iopub_stream_stdout(
        "Called from: eval(quote({\n    local_foo <- 1\n    browser()\n}), new.env())\n",
    );

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    let code = "local_foo";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // Should ideally be `recv_iopub_execute_result()`, but auto-printing
    // detection currently does not work reliably in debug REPLs
    frontend.recv_iopub_stream_stdout("[1] 1\n");
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
    frontend.recv_iopub_stream_stdout("Called from: evalq(browser(), baseenv())\n");

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // While paused in the debugger, evaluate a simple expression
    let code = "1 + 1";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_stream_stdout("[1] 2\n");

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
    frontend.recv_iopub_stream_stdout("Called from: evalq(base::browser(), env)\n");

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // While paused in the debugger, evaluate a simple expression that only
    // requires `::`
    let code = "base::list";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_stream_stdout("function (...)  .Primitive(\"list\")\n");

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
