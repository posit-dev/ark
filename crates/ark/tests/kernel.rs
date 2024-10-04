use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
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

    // TODO!: Why do we get the message as a stream?
    frontend.recv_iopub_stream_stderr("Error: \nCan't execute incomplete input:\n1 +\n");

    // assert!(frontend
    //     .recv_iopub_execute_error()
    //     .contains("Can't execute incomplete input"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);

    // assert_eq!(
    //     frontend.recv_shell_execute_reply_exception(),
    //     input.execution_count
    // );

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
fn test_execute_request_error_multiple_expressions() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_execute_request("1\nstop('foobar')\n2", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, "1\nstop('foobar')\n2");

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
