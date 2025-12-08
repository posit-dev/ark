use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark::fixtures::DummyArkFrontend;

#[test]
fn test_execute_request() {
    let frontend = DummyArkFrontend::lock();
    frontend.execute_request("42", |result| assert_eq!(result, "[1] 42"));
}

#[test]
fn test_execute_request_empty() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request_invisibly("");

    // Equivalent to invisible output
    frontend.execute_request_invisibly("invisible(1)");
}

#[test]
fn test_execute_request_multiple_lines() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request("1 +\n  2+\n  3", |result| assert_eq!(result, "[1] 6"));
}

#[test]
fn test_execute_request_incomplete() {
    // Set RUST_BACKTRACE to ensure backtraces are captured. We used to leak
    // backtraces in syntax error messages, and this shouldn't happen even when
    // `RUST_BACKTRACE` is set.
    std::env::set_var("RUST_BACKTRACE", "1");

    let frontend = DummyArkFrontend::lock();

    frontend.execute_request_invisibly("options(positron.error_entrace = FALSE)");

    frontend.execute_request_error("1 +", |error_msg| {
        assert_eq!(error_msg, "Error:\nCan't parse incomplete input");
    });
}

#[test]
fn test_execute_request_incomplete_multiple_lines() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request_error("1 +\n2 +", |error_msg| {
        assert!(error_msg.contains("Can't parse incomplete input"));
    });
}

#[test]
fn test_execute_request_invalid() {
    // Set RUST_BACKTRACE to ensure backtraces are captured. We used to leak
    // backtraces in syntax error messages, and this shouldn't happen even when
    // `RUST_BACKTRACE` is set.
    std::env::set_var("RUST_BACKTRACE", "1");

    let frontend = DummyArkFrontend::lock();

    frontend.execute_request_error("1 + )", |error_msg| {
        assert!(error_msg.contains("Syntax error"));
        assert!(!error_msg.contains("Stack backtrace:") && !error_msg.contains("std::backtrace"));
    });

    // https://github.com/posit-dev/ark/issues/598
    frontend.execute_request_error("``", |error_msg| {
        assert!(error_msg.contains("Syntax error"));
        assert!(!error_msg.contains("Stack backtrace:") && !error_msg.contains("std::backtrace"));
    });

    // https://github.com/posit-dev/ark/issues/722
    frontend.execute_request_error("_ + _()", |error_msg| {
        assert!(error_msg.contains("Syntax error"));
        assert!(!error_msg.contains("Stack backtrace:") && !error_msg.contains("std::backtrace"));
    });
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
    frontend.execute_request(code.as_str(), |result| assert!(result.contains(&aaa)));
}
