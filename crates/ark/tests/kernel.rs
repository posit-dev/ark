use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::recv_iopub_busy;
use amalthea::recv_iopub_execute_error;
use amalthea::recv_iopub_execute_input;
use amalthea::recv_iopub_execute_result;
use amalthea::recv_iopub_idle;
use amalthea::recv_iopub_stream_stderr;
use amalthea::recv_iopub_stream_stdout;
use amalthea::recv_shell_execute_reply;
use amalthea::recv_shell_execute_reply_exception;
use amalthea::recv_stdin_input_request;
use amalthea::wire::jupyter_message::Message;
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

    recv_iopub_busy!(frontend);
    recv_iopub_idle!(frontend);
}

#[test]
fn test_execute_request() {
    let frontend = DummyArkFrontend::lock();

    let code = "42";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    assert_eq!(recv_iopub_execute_result!(frontend), "[1] 42");

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_execute_request_empty() {
    let frontend = DummyArkFrontend::lock();

    let code = "";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    // Equivalent to invisible output
    let code = "invisible(1)";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_execute_request_multiple_lines() {
    let frontend = DummyArkFrontend::lock();

    let code = "1 +\n  2+\n  3";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    assert_eq!(recv_iopub_execute_result!(frontend), "[1] 6");

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count)
}

#[test]
fn test_execute_request_incomplete() {
    let frontend = DummyArkFrontend::lock();

    let code = "1 +";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    assert!(recv_iopub_execute_error!(frontend).contains("Can't execute incomplete input"));

    recv_iopub_idle!(frontend);

    assert_eq!(
        recv_shell_execute_reply_exception!(frontend),
        input.execution_count
    )
}

#[test]
fn test_execute_request_incomplete_multiple_lines() {
    let frontend = DummyArkFrontend::lock();

    let code = "1 +\n2 +";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    assert!(recv_iopub_execute_error!(frontend).contains("Can't execute incomplete input"));

    recv_iopub_idle!(frontend);

    assert_eq!(
        recv_shell_execute_reply_exception!(frontend),
        input.execution_count
    )
}

#[test]
fn test_execute_request_browser() {
    let frontend = DummyArkFrontend::lock();

    let code = "browser()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    assert!(recv_iopub_execute_result!(frontend).contains("Called from: top level"));

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
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
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    assert!(recv_iopub_execute_result!(frontend).contains("Called from: top level"));

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    frontend.send_execute_request("stop('foobar')", ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, "stop('foobar')");

    recv_iopub_stream_stderr!(frontend, "Error: foobar\n");
    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_execute_request_browser_incomplete() {
    let frontend = DummyArkFrontend::lock();

    let code = "browser()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    assert!(recv_iopub_execute_result!(frontend).contains("Called from: top level"));

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    let code = "1 +";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_stream_stderr!(frontend, "Error: \nCan't execute incomplete input:\n1 +\n");
    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
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
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    // We aren't at top level, so this comes as an iopub stream
    recv_iopub_stream_stdout!(frontend, "Called from: fn()\n");
    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    // Execute a multiline statement while paused in the debugger
    let code = "1 +
        1";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    // Also received as iopub stream because we aren't at top level, we are in the debugger
    recv_iopub_stream_stdout!(frontend, "[1] 2\n");
    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_execute_request_browser_stdin() {
    let frontend = DummyArkFrontend::lock();

    let code = "browser()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    assert!(recv_iopub_execute_result!(frontend).contains("Called from: top level"));

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    let options = ExecuteRequestOptions { allow_stdin: true };
    let code = "readline('prompt>')";
    frontend.send_execute_request(code, options);
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    let prompt = recv_stdin_input_request!(frontend);
    assert_eq!(prompt, String::from("prompt>"));

    frontend.send_stdin_input_reply(String::from("hi"));

    assert_eq!(recv_iopub_execute_result!(frontend), "[1] \"hi\"");
    recv_iopub_idle!(frontend);
    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);

    let code = "Q";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_execute_request_error() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_execute_request("stop('foobar')", ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, "stop('foobar')");
    assert!(recv_iopub_execute_error!(frontend).contains("foobar"));

    recv_iopub_idle!(frontend);

    assert_eq!(
        recv_shell_execute_reply_exception!(frontend),
        input.execution_count
    );
}

#[test]
fn test_execute_request_error_multiple_expressions() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_execute_request("1\nstop('foobar')\n2", ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, "1\nstop('foobar')\n2");

    recv_iopub_stream_stdout!(frontend, "[1] 1\n");
    assert!(recv_iopub_execute_error!(frontend).contains("foobar"));

    recv_iopub_idle!(frontend);

    assert_eq!(
        recv_shell_execute_reply_exception!(frontend),
        input.execution_count
    );
}

#[test]
fn test_execute_request_multiple_expressions() {
    let frontend = DummyArkFrontend::lock();

    let code = "1\nprint(2)\n3";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    // Printed output
    recv_iopub_stream_stdout!(frontend, "[1] 1\n[1] 2\n");

    // In console mode, we get output for all intermediate results.  That's not
    // the case in notebook mode where only the final result is emitted. Note
    // that `print()` returns invisibly.
    assert_eq!(recv_iopub_execute_result!(frontend), "[1] 3");

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_execute_request_single_line_buffer_overflow() {
    let frontend = DummyArkFrontend::lock();

    // The newlines do matter for what we are testing here,
    // due to how we internally split by newlines. We want
    // to test that the `aaa`s result in an immediate R error,
    // not in text written to the R buffer that calls `stop()`.
    let aaa = "a".repeat(4096);
    let code = format!("quote(\n{aaa}\n)");
    frontend.send_execute_request(code.as_str(), ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    assert!(recv_iopub_execute_error!(frontend).contains("Can't pass console input on to R"));

    recv_iopub_idle!(frontend);

    assert_eq!(
        recv_shell_execute_reply_exception!(frontend),
        input.execution_count
    );
}

#[test]
fn test_stdin_basic_prompt() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions { allow_stdin: true };

    let code = "readline('prompt>')";
    frontend.send_execute_request(code, options);
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    let prompt = recv_stdin_input_request!(frontend);
    assert_eq!(prompt, String::from("prompt>"));

    frontend.send_stdin_input_reply(String::from("hi"));

    assert_eq!(recv_iopub_execute_result!(frontend), "[1] \"hi\"");

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_stdin_followed_by_an_expression_on_the_same_line() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions { allow_stdin: true };

    let code = "val <- readline('prompt>'); paste0(val,'-there')";
    frontend.send_execute_request(code, options);
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    let prompt = recv_stdin_input_request!(frontend);
    assert_eq!(prompt, String::from("prompt>"));

    frontend.send_stdin_input_reply(String::from("hi"));

    assert_eq!(recv_iopub_execute_result!(frontend), "[1] \"hi-there\"");

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_stdin_followed_by_an_expression_on_the_next_line() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions { allow_stdin: true };

    let code = "1\nval <- readline('prompt>')\npaste0(val,'-there')";
    frontend.send_execute_request(code, options);
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_stream_stdout!(frontend, "[1] 1\n");

    let prompt = recv_stdin_input_request!(frontend);
    assert_eq!(prompt, String::from("prompt>"));

    frontend.send_stdin_input_reply(String::from("hi"));

    assert_eq!(recv_iopub_execute_result!(frontend), "[1] \"hi-there\"");

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_stdin_single_line_buffer_overflow() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions { allow_stdin: true };

    let code = "1\nreadline('prompt>')";
    frontend.send_execute_request(code, options);
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    recv_iopub_stream_stdout!(frontend, "[1] 1\n");

    let prompt = recv_stdin_input_request!(frontend);
    assert_eq!(prompt, String::from("prompt>"));

    // Would overflow R's internal buffer
    let aaa = "a".repeat(4096);
    frontend.send_stdin_input_reply(aaa);

    assert!(recv_iopub_execute_error!(frontend).contains("Can't pass console input on to R"));

    recv_iopub_idle!(frontend);

    assert_eq!(
        recv_shell_execute_reply_exception!(frontend),
        input.execution_count
    );
}

#[test]
fn test_stdin_from_menu() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions { allow_stdin: true };

    let code = "menu(c('a', 'b'))\n3";
    frontend.send_execute_request(code, options);
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    // R emits this before asking for your selection
    recv_iopub_stream_stdout!(
        frontend,
        "
1: a
2: b

"
    );

    let prompt = recv_stdin_input_request!(frontend);
    assert_eq!(prompt, String::from("Selection: "));

    frontend.send_stdin_input_reply(String::from("b"));

    // Position of selection is returned
    recv_iopub_stream_stdout!(frontend, "[1] 2\n");

    assert_eq!(recv_iopub_execute_result!(frontend), "[1] 3");

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
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
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);
    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}
