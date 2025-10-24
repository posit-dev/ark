use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::recv_iopub_busy;
use amalthea::recv_iopub_execute_error;
use amalthea::recv_iopub_execute_input;
use amalthea::recv_iopub_execute_result;
use amalthea::recv_iopub_idle;
use amalthea::recv_iopub_stream_stdout;
use amalthea::recv_shell_execute_reply;
use amalthea::recv_shell_execute_reply_exception;
use amalthea::recv_stdin_input_request;
use ark::fixtures::DummyArkFrontendNotebook;

#[test]
fn test_notebook_execute_request() {
    let frontend = DummyArkFrontendNotebook::lock();

    frontend.send_execute_request("42", ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, "42");
    assert_eq!(recv_iopub_execute_result!(frontend), "[1] 42");

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_execute_request_error_multiple_expressions() {
    let frontend = DummyArkFrontendNotebook::lock();

    frontend.send_execute_request("1\nstop('foobar')\n2", ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, "1\nstop('foobar')\n2");

    assert!(recv_iopub_execute_error!(frontend).contains("foobar"));

    recv_iopub_idle!(frontend);

    assert_eq!(
        recv_shell_execute_reply_exception!(frontend),
        input.execution_count
    );
}

#[test]
fn test_notebook_execute_request_multiple_expressions() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "1\nprint(2)\n3";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    recv_iopub_busy!(frontend);

    let input = recv_iopub_execute_input!(frontend);
    assert_eq!(input.code, code);

    // Printed output
    recv_iopub_stream_stdout!(frontend, "[1] 2\n");

    // Unlike console mode, we don't get intermediate results in notebooks
    assert_eq!(recv_iopub_execute_result!(frontend), "[1] 3");

    recv_iopub_idle!(frontend);

    assert_eq!(recv_shell_execute_reply!(frontend), input.execution_count);
}

#[test]
fn test_notebook_execute_request_incomplete() {
    let frontend = DummyArkFrontendNotebook::lock();

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
fn test_notebook_execute_request_incomplete_multiple_lines() {
    let frontend = DummyArkFrontendNotebook::lock();

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
fn test_notebook_stdin_basic_prompt() {
    let frontend = DummyArkFrontendNotebook::lock();

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
fn test_notebook_stdin_followed_by_an_expression_on_the_same_line() {
    let frontend = DummyArkFrontendNotebook::lock();

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
fn test_notebook_stdin_followed_by_an_expression_on_the_next_line() {
    let frontend = DummyArkFrontendNotebook::lock();

    let options = ExecuteRequestOptions { allow_stdin: true };

    // Note, `1` is an intermediate output and is not emitted in notebooks
    let code = "1\nval <- readline('prompt>')\npaste0(val,'-there')";
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
