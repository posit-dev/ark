use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark::fixtures::DummyArkFrontend;

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

    frontend.recv_iopub_stream_stdout("[1] 1\n");
    assert!(frontend.recv_iopub_execute_error().contains("foobar"));

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

    frontend.recv_iopub_stream_stderr(
        r#"The `getOption("error")` handler failed.
This option was unset to avoid cascading errors.
Caused by:
ouch
"#,
    );

    assert!(frontend.recv_iopub_execute_error().contains("foo"));

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

    frontend.recv_iopub_stream_stdout("Enter an item from the menu, or 0 to exit\n");

    frontend.recv_iopub_stream_stderr(
        r#"The `getOption("error")` handler failed.
This option was unset to avoid cascading errors.
Caused by:
Can't request input from the user at this time.
Are you calling `readline()` or `menu()` from `options(error = )`?
"#,
    );

    assert!(frontend.recv_iopub_execute_error().contains("foo"));
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

    // We set up the call stack to show a simple `error_handler()`
    frontend.recv_iopub_stream_stdout("Called from: ark_recover()\n");

    assert!(frontend.recv_iopub_execute_error().contains("foo"));

    frontend.recv_iopub_idle();
    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );
}
