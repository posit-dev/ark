use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

#[test]
fn test_stdin_basic_prompt() {
    let frontend = DummyArkFrontend::lock();

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
fn test_stdin_followed_by_an_expression_on_the_same_line() {
    let frontend = DummyArkFrontend::lock();

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
fn test_stdin_followed_by_an_expression_on_the_next_line() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };

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

#[test]
fn test_stdin_single_line_buffer_overflow() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };

    let code = "1\nreadline('prompt>')";
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    let prompt = frontend.recv_stdin_input_request();
    assert_eq!(prompt, String::from("prompt>"));

    // Would overflow R's internal buffer
    let aaa = "a".repeat(4096);
    frontend.send_stdin_input_reply(aaa);

    assert!(frontend
        .recv_iopub_execute_error()
        .contains("Can't pass console input on to R"));

    frontend.assert_stream_stdout_contains("[1] 1");
    frontend.recv_iopub_idle();

    assert_eq!(
        frontend.recv_shell_execute_reply_exception(),
        input.execution_count
    );
}

#[test]
fn test_stdin_from_menu() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };

    let code = "menu(c('a', 'b'))\n3";
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    let prompt = frontend.recv_stdin_input_request();
    assert_eq!(prompt, String::from("Selection: "));

    frontend.send_stdin_input_reply(String::from("b"));

    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 3");

    // R emits menu options before asking for selection, then the selection result
    frontend.assert_stream_stdout_contains("1: a");
    frontend.assert_stream_stdout_contains("2: b");
    frontend.assert_stream_stdout_contains("[1] 2");

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_stdin_readline_during_autoprint() {
    let frontend = DummyArkFrontend::lock();

    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };

    // The print method writes output with `cat()` then calls `readline()`.
    // Since the object is the last expression, R auto-prints it via an
    // inlined `print()` call. `is_auto_printing()` returns true so `cat()`
    // output gets buffered in `autoprint_output`. That buffer must be
    // flushed as stream stdout before the input request is sent to the
    // frontend, otherwise the user sees only the bare prompt without the
    // preceding question text (https://github.com/posit-dev/positron/issues/12688).
    let code =
        "print.test_input <- function(x, ...) { cat('question?\\n'); readline('prompt>') }\nstructure(1, class = 'test_input')";
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();

    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // The `cat()` output from the print method must appear as stream stdout
    frontend.assert_stream_stdout_contains("question?");

    let prompt = frontend.recv_stdin_input_request();
    assert_eq!(prompt, String::from("prompt>"));

    frontend.send_stdin_input_reply(String::from("hi"));

    frontend.recv_iopub_idle();

    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

/// `r_task()` calls from non-R threads are intentionally NOT processed while R
/// is at an input-request prompt (e.g. `readline()` / `menu()`). The task
/// channels drained inside `run_event_loop()` are gated to top-level and
/// browser prompts only. A pending `r_task()` waits until R returns to one of
/// those prompts before running.
///
/// The goal of not running tasks at the input request prompt is to avoid too
/// much reentrancy risk.
#[test]
fn test_r_task_does_not_run_at_input_request_prompt() {
    let frontend = DummyArkFrontend::lock();

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

    // R is now blocked at the input-request prompt. Issue an `r_task()` from
    // a worker thread. The closure is trivial, so if tasks were drained here
    // it would complete near-instantly.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let value = ark::r_task::r_task(|| 42);
        let _ = tx.send(value);
    });

    // The task should NOT complete while R is at the input-request prompt.
    assert_eq!(
        rx.recv_timeout(std::time::Duration::from_millis(500)),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    );

    // Respond to the input-request prompt so R returns to the top-level prompt
    frontend.send_stdin_input_reply(String::from("hi"));
    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Once R is back at the top-level prompt, the queued task should run.
    let value = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("`r_task()` should run once R returns to a top-level prompt");
    assert_eq!(value, 42);
}
