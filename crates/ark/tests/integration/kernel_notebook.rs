use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontendNotebook;

#[test]
fn test_notebook_execute_request() {
    let frontend = DummyArkFrontendNotebook::lock();

    frontend.send_execute_request("42", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, "42");
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 42");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_execute_request_error_multiple_expressions() {
    let frontend = DummyArkFrontendNotebook::lock();

    frontend.send_execute_request("1\nstop('foobar')\n2", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, "1\nstop('foobar')\n2");

    assert!(frontend.recv_iopub_execute_error().contains("foobar"));

    // The intermediate expression evaluated before the error still streams
    // its visible value
    frontend.assert_stream_stdout_contains("[1] 1");

    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );
}

#[test]
fn test_notebook_execute_request_multiple_expressions() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "1\nprint(2)\n3";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // As in console mode, intermediate results are streamed on stdout and
    // only the last expression becomes the execute result. Note that
    // `print()` returns invisibly.
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 3");

    // Intermediate autoprint and printed output
    frontend.assert_stream_stdout_contains("[1] 1");
    frontend.assert_stream_stdout_contains("[1] 2");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_notebook_execute_request_intermediate_autoprint() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "1\n2\n3";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // The last expression's value is the execute result
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 3");

    // The intermediate expressions' values are streamed on stdout, and the
    // last expression's value is not duplicated there
    let streams = frontend.recv_iopub_idle_and_flush();
    assert_eq!(streams.stdout(), "[1] 1\n[1] 2\n");

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_notebook_execute_request_invisible_intermediate_expression() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "invisible(1)\n2";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 2");

    // The invisible intermediate expression produces no stream output
    let streams = frontend.recv_iopub_idle_and_flush();
    assert_eq!(streams.stdout(), "");

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_notebook_execute_request_incomplete() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "1 +";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_error()
        .contains("Can't parse incomplete input"));

    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    )
}

#[test]
fn test_notebook_execute_request_incomplete_multiple_lines() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "1 +\n2 +";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    assert!(frontend
        .recv_iopub_execute_error()
        .contains("Can't parse incomplete input"));

    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    )
}

#[test]
fn test_notebook_stdin_basic_prompt() {
    let frontend = DummyArkFrontendNotebook::lock();

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
}

#[test]
fn test_notebook_stdin_followed_by_an_expression_on_the_same_line() {
    let frontend = DummyArkFrontendNotebook::lock();

    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };

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
fn test_notebook_execute_request_data_frame() {
    let frontend = DummyArkFrontendNotebook::lock();

    frontend.send_execute_request(
        "data.frame(x = 1:3, y = 4:6)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let result_data = frontend.recv_iopub_execute_result_data();

    let plain = result_data["text/plain"].as_str().unwrap();
    assert_eq!(plain, "  x y\n1 1 4\n2 2 5\n3 3 6");

    assert!(!result_data.contains_key("text/html"));

    // Vanilla notebook mode: no inline data explorer MIME
    assert!(!result_data.contains_key("application/vnd.positron.dataExplorer+json"));

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_notebook_stdin_followed_by_an_expression_on_the_next_line() {
    let frontend = DummyArkFrontendNotebook::lock();

    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };

    // Note, `1` is an intermediate expression whose value is streamed on stdout
    let code = "1\nval <- readline('prompt>')\npaste0(val,'-there')";
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    let prompt = frontend.recv_stdin_input_request();
    assert_eq!(prompt, String::from("prompt>"));

    frontend.send_stdin_input_reply(String::from("hi"));

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] \"hi-there\"");

    frontend.assert_stream_stdout_contains("[1] 1");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}
