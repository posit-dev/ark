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
#[cfg(unix)]
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
#[cfg(unix)]
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

static SHUTDOWN_TESTS_ENABLED: bool = false;

// Can shut down Ark when running a nested debug console
// https://github.com/posit-dev/positron/issues/6553
#[test]
#[cfg(unix)]
fn test_shutdown_request_browser() {
    if !SHUTDOWN_TESTS_ENABLED {
        return;
    }

    install_sigint_handler();
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request("browser()", |result| {
        assert!(result.contains("Called from: top level"));
    });

    frontend.send_shutdown_request(true);
    frontend.recv_iopub_busy();

    // There is a race condition between the Control thread and the Shell
    // threads. Ideally we'd wait for both the Shutdown reply and the IOPub Idle
    // messages concurrently instead of sequentially.
    let reply = frontend.recv_control_shutdown_reply();
    assert_eq!(reply.status, Status::Ok);
    assert_eq!(reply.restart, true);

    frontend.recv_iopub_idle();

    DummyArkFrontend::wait_for_cleanup();
}

#[test]
#[cfg(unix)]
fn test_shutdown_request_while_busy() {
    if !SHUTDOWN_TESTS_ENABLED {
        return;
    }

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

    // It seems this isn't emitted on older R versions
    frontend.recv_iopub_stream_stderr("\n");
    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
    frontend.recv_iopub_idle();

    DummyArkFrontend::wait_for_cleanup();
}

#[test]
fn test_execute_request_source_references() {
    let frontend = DummyArkFrontend::lock();

    // Test that our parser attaches source references when global option is set
    frontend.execute_request_invisibly("options(keep.source = TRUE)");
    frontend.execute_request_invisibly("f <- function() {}");

    frontend.execute_request(
        "srcref <- attr(f, 'srcref'); inherits(srcref, 'srcref')",
        |result| {
            assert_eq!(result, "[1] TRUE");
        },
    );

    frontend.execute_request(
        "srcfile <- attr(srcref, 'srcfile'); inherits(srcfile, 'srcfile')",
        |result| {
            assert_eq!(result, "[1] TRUE");
        },
    );

    // When global option is unset, we don't attach source references
    frontend.execute_request_invisibly("options(keep.source = FALSE)");
    frontend.execute_request_invisibly("g <- function() {}");

    frontend.execute_request(
        "srcref <- attr(g, 'srcref'); identical(srcref, NULL)",
        |result| {
            assert_eq!(result, "[1] TRUE");
        },
    );
}

#[test]
fn test_platform_gui_positron_console() {
    let frontend = DummyArkFrontend::lock();

    // Console sessions should have .Platform$GUI set to "Positron"
    frontend.execute_request(".Platform$GUI", |result| {
        assert_eq!(result, "[1] \"Positron\"");
    });
}
