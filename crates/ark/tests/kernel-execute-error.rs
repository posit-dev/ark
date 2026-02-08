use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

#[test]
fn test_execute_request_error() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request_error("stop('foobar')", |error_msg| {
        assert!(error_msg.contains("foobar"));
    });
}

#[test]
fn test_execute_request_error_with_accumulated_output() {
    // Test that when the very last input throws an error after producing
    // output, the accumulated output is flushed before the error is reported.
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

    // Then the error should be reported on stderr
    assert!(frontend.recv_iopub_execute_error().contains("foo"));

    // The output from print(1) should be flushed to stdout
    frontend.assert_stream_stdout_contains("[1] 42");

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
    frontend.execute_request_error(
        "options(expressions = 100); f <- function(x) if (x > 0 ) f(x - 1); f(100)",
        |error_msg| {
            assert!(error_msg.contains("evaluation nested too deeply"));
        },
    );

    // Check we can still evaluate without causing another too deeply nested error
    frontend.execute_request_invisibly("f(10)");
}

#[test]
fn test_execute_request_error_expressions_overflow_last_value() {
    let frontend = DummyArkFrontend::lock();

    // Set state and last value
    frontend.execute_request_invisibly(
        "options(expressions = 100); f <- function(x) if (x > 0 ) f(x - 1); invisible('hello')",
    );

    // Check last value is set
    frontend.execute_request(".Last.value", |result| {
        assert_eq!(result, "[1] \"hello\"");
    });

    // Deterministically produce an "evaluation too deeply nested" error
    frontend.execute_request_error("f(100)", |error_msg| {
        assert!(error_msg.contains("evaluation nested too deeply"));
    });

    // Check last value is still set
    frontend.execute_request(".Last.value", |result| {
        assert_eq!(result, "[1] \"hello\"");
    });
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

    assert!(frontend.recv_iopub_execute_error().contains("foobar"));

    frontend.assert_stream_stdout_contains("[1] 1");
    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );
}

#[test]
fn test_execute_request_error_handler_failure() {
    let frontend = DummyArkFrontend::lock();

    let code = r#"
f <- function() g()
g <- function() h()
h <- function() stop("foo")
options(error = function() stop("ouch"))
"#;
    frontend.execute_request_invisibly(code);

    frontend.send_execute_request("f()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, "f()");

    assert!(frontend.recv_iopub_execute_error().contains("foo"));

    frontend.assert_stream_stderr_contains("The `getOption(\"error\")` handler failed.");
    frontend.recv_iopub_idle();
    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );
}

#[test]
fn test_execute_request_error_handler_readline() {
    let frontend = DummyArkFrontend::lock();

    let code = r#"
f <- function() g()
g <- function() h()
h <- function() stop("foo")
options(error = function() menu("ouch"))
"#;
    frontend.execute_request_invisibly(code);

    frontend.send_execute_request("f()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, "f()");

    assert!(frontend.recv_iopub_execute_error().contains("foo"));

    frontend.assert_stream_stdout_contains("Enter an item from the menu, or 0 to exit");
    frontend.assert_stream_stderr_contains("The `getOption(\"error\")` handler failed.");
    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );
}

#[test]
fn test_execute_request_error_recover() {
    let frontend = DummyArkFrontend::lock();

    let code = r#"
f <- function() g()
g <- function() h()
h <- function() stop("foo")
options(error = recover)
"#;
    frontend.execute_request_invisibly(code);

    frontend.send_execute_request("f()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, "f()");

    assert!(frontend.recv_iopub_execute_error().contains("foo"));

    // We set up the call stack to show a simple `error_handler()`
    frontend.assert_stream_stdout_contains("Called from: ark_recover()");

    frontend.recv_iopub_idle();
    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );
}
