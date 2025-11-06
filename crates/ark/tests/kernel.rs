use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use ark::fixtures::DummyArkFrontend;
use stdext::assert_match;

#[test]
fn test_kernel_info() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_shell(KernelInfoRequest {});

    assert_match!(frontend.recv_shell(), Message::KernelInfoReply(reply) => {
        assert_eq!(reply.content.language_info.name, "R");
        assert_eq!(reply.content.language_info.pygments_lexer, None);
        assert_eq!(reply.content.language_info.codemirror_mode, None);
        assert_eq!(reply.content.language_info.nbconvert_exporter, None);
    });

    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
}

#[test]
fn test_execute_request() {
    let frontend = DummyArkFrontend::lock();

    let code = "42";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 42");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_execute_request_empty() {
    let frontend = DummyArkFrontend::lock();

    let code = "";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Equivalent to invisible output
    let code = "invisible(1)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_execute_request_multiple_lines() {
    let frontend = DummyArkFrontend::lock();

    let code = "1 +\n  2+\n  3";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 6");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count)
}

#[test]
fn test_execute_request_incomplete() {
    let frontend = DummyArkFrontend::lock();

    let code = "1 +";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_error()
        .contains("Can't execute incomplete input"));

    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    )
}

#[test]
fn test_execute_request_incomplete_multiple_lines() {
    let frontend = DummyArkFrontend::lock();

    let code = "1 +\n2 +";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_error()
        .contains("Can't execute incomplete input"));

    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    )
}

#[test]
fn test_execute_request_invalid() {
    let frontend = DummyArkFrontend::lock();

    let code = "1 + )";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend.recv_iopub_execute_error().contains("Syntax error"));

    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    )
}

#[test]
fn test_execute_request_browser() {
    let frontend = DummyArkFrontend::lock();

    let code = "browser()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_result()
        .contains("Called from: top level"));

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
fn test_execute_request_browser_continue() {
    let frontend = DummyArkFrontend::lock();

    let code = "browser()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_result()
        .contains("Called from: top level"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    let code = "n";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_execute_request_browser_nested() {
    // Test nested browser() calls - entering a browser within a browser
    let frontend = DummyArkFrontend::lock();

    // Start first browser
    let code = "browser()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_result()
        .contains("Called from: top level"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Evaluate a value in the outer browser
    let code = "42";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend.recv_iopub_execute_result().contains("[1] 42"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Start nested browser from within the first browser
    let code = "browser()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // Nested browser() produces execute_result output
    frontend.recv_iopub_execute_result();

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Evaluate a command in the nested browser
    let code = "1";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend.recv_iopub_execute_result().contains("[1] 1"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Evaluate another value in the nested browser
    let code = "\"hello\"";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend.recv_iopub_execute_result().contains("hello"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

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
    let code = "c";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Back in the parent browser, evaluate another value
    let code = "3.14";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend.recv_iopub_execute_result().contains("[1] 3.14"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Throw an error in the outer browser
    let code = "stop('error in parent')";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_stream_stderr("Error: error in parent\n");
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    let code = "NA";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend.recv_iopub_execute_result().contains("[1] NA"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
    // Quit the outer browser
    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_execute_request_browser_error() {
    // The behaviour for errors is different in browsers than at top-level
    // because our global handler does not run in that case. Instead the error
    // is streamed on IOPub::Stderr and a regular execution result is sent as
    // response.

    let frontend = DummyArkFrontend::lock();

    let code = "browser()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_result()
        .contains("Called from: top level"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    frontend.send_execute_request("stop('foobar')", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, "stop('foobar')");

    frontend.recv_iopub_stream_stderr("Error: foobar\n");
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
fn test_execute_request_browser_incomplete() {
    let frontend = DummyArkFrontend::lock();

    let code = "browser()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_result()
        .contains("Called from: top level"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    let code = "1 +";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_stream_stderr("Error: Can't execute incomplete input:\n1 +\n");
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

    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_execute_request_browser_stdin() {
    let frontend = DummyArkFrontend::lock();

    let code = "browser()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_result()
        .contains("Called from: top level"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

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

    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
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

    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_execute_request_error() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_execute_request("stop('foobar')", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, "stop('foobar')");
    assert!(frontend.recv_iopub_execute_error().contains("foobar"));

    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );
}

#[test]
fn test_execute_request_error_with_accumulated_output() {
    // Test that when the very last input output and then throws an error,
    // the accumulated output is flushed before the error is reported.
    // This tests the autoprint buffer flush logic in error handling.
    let frontend = DummyArkFrontend::lock();

    let code = "{
        print.foo <- function(x) {
            print(unclass(x))
            stop(\"foo\")
        }
        structure(42, class = \"foo\")
    }";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // The output from print(1) should be flushed to stdout
    frontend.recv_iopub_stream_stdout("[1] 42\n");

    // Then the error should be reported on stderr
    assert!(frontend.recv_iopub_execute_error().contains("foo"));
    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );
}

#[test]
fn test_execute_request_error_expressions_overflow() {
    let frontend = DummyArkFrontend::lock();
    // Deterministically produce an "evaluation too deeply nested" error
    let code = "options(expressions = 100); f <- function(x) if (x > 0 ) f(x - 1); f(100)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_error()
        .contains("evaluation nested too deeply"));

    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );

    // Check we can still evaluate without causing another too deeply nested error
    let code = "f(10)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_execute_request_error_expressions_overflow_last_value() {
    let frontend = DummyArkFrontend::lock();

    // Set state and last value
    let code =
        "options(expressions = 100); f <- function(x) if (x > 0 ) f(x - 1); invisible('hello')";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Check last value is set
    let code = ".Last.value";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] \"hello\"");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // Deterministically produce an "evaluation too deeply nested" error
    let code = "f(100)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_error()
        .contains("evaluation nested too deeply"));

    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );

    // Check last value is still set
    let code = ".Last.value";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] \"hello\"");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_execute_request_error_multiple_expressions() {
    let frontend = DummyArkFrontend::lock();

    // `print(2)` and `3` are never evaluated
    let code = "1\nstop('foobar')\nprint(2)\n3";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_stream_stdout("[1] 1\n");
    assert!(frontend.recv_iopub_execute_error().contains("foobar"));

    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );
}

#[test]
fn test_execute_request_multiple_expressions() {
    let frontend = DummyArkFrontend::lock();

    let code = "1\nprint(2)\n3";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // Printed output
    frontend.recv_iopub_stream_stdout("[1] 1\n[1] 2\n");

    // In console mode, we get output for all intermediate results.  That's not
    // the case in notebook mode where only the final result is emitted. Note
    // that `print()` returns invisibly.
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 3");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_execute_request_single_line_buffer_overflow() {
    let frontend = DummyArkFrontend::lock();

    // This used to fail back when we were passing inputs down to the REPL from
    // our `ReadConsole` handler. Below is the old test description for posterity.

    // The newlines do matter for what we are testing here,
    // due to how we internally split by newlines. We want
    // to test that the `aaa`s result in an immediate R error,
    // not in text written to the R buffer that calls `stop()`.
    let aaa = "a".repeat(4096);
    let code = format!("quote(\n{aaa}\n)");
    frontend.send_execute_request(code.as_str(), ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend.recv_iopub_execute_result().contains(&aaa));

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_stdin_basic_prompt() {
    let frontend = DummyArkFrontend::lock();

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
}

#[test]
fn test_stdin_followed_by_an_expression_on_the_same_line() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions { allow_stdin: true };

    let code = "val <- readline('prompt>'); paste0(val,'-there')";
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    let prompt = frontend.recv_stdin_input_request();
    assert_eq!(prompt, String::from("prompt>"));

    frontend.send_stdin_input_reply(String::from("hi"));

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] \"hi-there\"");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_stdin_followed_by_an_expression_on_the_next_line() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions { allow_stdin: true };

    let code = "1\nval <- readline('prompt>')\npaste0(val,'-there')";
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_stream_stdout("[1] 1\n");

    let prompt = frontend.recv_stdin_input_request();
    assert_eq!(prompt, String::from("prompt>"));

    frontend.send_stdin_input_reply(String::from("hi"));

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] \"hi-there\"");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_stdin_single_line_buffer_overflow() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions { allow_stdin: true };

    let code = "1\nreadline('prompt>')";
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.recv_iopub_stream_stdout("[1] 1\n");

    let prompt = frontend.recv_stdin_input_request();
    assert_eq!(prompt, String::from("prompt>"));

    // Would overflow R's internal buffer
    let aaa = "a".repeat(4096);
    frontend.send_stdin_input_reply(aaa);

    assert!(frontend
        .recv_iopub_execute_error()
        .contains("Can't pass console input on to R"));

    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );
}

#[test]
fn test_stdin_from_menu() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions { allow_stdin: true };

    let code = "menu(c('a', 'b'))\n3";
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // R emits this before asking for your selection
    frontend.recv_iopub_stream_stdout(
        "
1: a
2: b

",
    );

    let prompt = frontend.recv_stdin_input_request();
    assert_eq!(prompt, String::from("Selection: "));

    frontend.send_stdin_input_reply(String::from("b"));

    // Position of selection is returned
    frontend.recv_iopub_stream_stdout("[1] 2\n");

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 3");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
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

#[test]
fn test_env_vars() {
    // These environment variables are set by R's shell script frontend.
    // We set these in Ark as well.
    let frontend = DummyArkFrontend::lock();

    let code = "stopifnot(
            identical(Sys.getenv('R_SHARE_DIR'), R.home('share')),
            identical(Sys.getenv('R_INCLUDE_DIR'), R.home('include')),
            identical(Sys.getenv('R_DOC_DIR'), R.home('doc'))
        )";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

/// Install a SIGINT handler for shutdown tests. This overrides the test runner
/// handler so it doesn't cancel our test.
fn install_sigint_handler() {
    extern "C" fn sigint_handler(_: libc::c_int) {}
    #[cfg(unix)]
    unsafe {
        use nix::sys::signal::signal;
        use nix::sys::signal::SigHandler;
        use nix::sys::signal::Signal;

        signal(Signal::SIGINT, SigHandler::Handler(sigint_handler)).unwrap();
    }
}

// Note that because of these shutdown tests you _have_ to use `cargo nextest`
// instead of `cargo test`, so that each test has its own process and R thread.
#[test]
fn test_shutdown_request() {
    install_sigint_handler();
    let frontend = DummyArkFrontend::lock();

    frontend.send_shutdown_request(false);
    frontend.recv_iopub_busy();

    let reply = frontend.recv_control_shutdown_reply();
    assert_eq!(reply.status, Status::Ok);
    assert_eq!(reply.restart, false);

    frontend.recv_iopub_idle();

    DummyArkFrontend::wait_for_cleanup();
}

#[test]
fn test_shutdown_request_with_restart() {
    install_sigint_handler();
    let frontend = DummyArkFrontend::lock();

    frontend.send_shutdown_request(true);
    frontend.recv_iopub_busy();

    let reply = frontend.recv_control_shutdown_reply();
    assert_eq!(reply.status, Status::Ok);
    assert_eq!(reply.restart, true);

    frontend.recv_iopub_idle();

    DummyArkFrontend::wait_for_cleanup();
}

// Can shut down Ark when running a nested debug console
// https://github.com/posit-dev/positron/issues/6553
#[test]
fn test_shutdown_request_browser() {
    install_sigint_handler();
    let frontend = DummyArkFrontend::lock();

    let code = "browser()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_result()
        .contains("Called from: top level"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    frontend.send_shutdown_request(true);
    frontend.recv_iopub_busy();

    let reply = frontend.recv_control_shutdown_reply();
    assert_eq!(reply.status, Status::Ok);
    assert_eq!(reply.restart, true);

    frontend.recv_iopub_idle();

    DummyArkFrontend::wait_for_cleanup();
}

#[test]
fn test_shutdown_request_while_busy() {
    install_sigint_handler();
    let frontend = DummyArkFrontend::lock();

    let code = "Sys.sleep(10)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    frontend.send_shutdown_request(false);
    frontend.recv_iopub_busy();

    let reply = frontend.recv_control_shutdown_reply();
    assert_eq!(reply.status, Status::Ok);
    assert_eq!(reply.restart, false);

    frontend.recv_iopub_stream_stderr("\n");
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
    frontend.recv_iopub_idle();

    DummyArkFrontend::wait_for_cleanup();
}
