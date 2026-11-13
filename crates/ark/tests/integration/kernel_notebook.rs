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

    // Unlike console mode, we don't get intermediate results in notebooks
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 3");

    // Printed output
    frontend.assert_stream_stdout_contains("[1] 2");

    frontend.recv_iopub_idle();

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

    // Note, `1` is an intermediate output and is not emitted in notebooks
    let code = "1\nval <- readline('prompt>')\npaste0(val,'-there')";
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
fn test_notebook_graphics_device_is_not_interactive() {
    let frontend = DummyArkFrontendNotebook::lock();

    // In notebooks, the Ark device is not an interactive device. Otherwise
    // `plot.lm()` and others prompt "Hit <Return> to see next plot" between
    // pages, and a notebook frontend has no good way to answer
    // (posit-dev/positron#14534).
    let code = "x <- .ark.graphics.device(); grDevices::dev.interactive()";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] FALSE");
    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_notebook_plot_lm_shows_all_pages_without_prompting() {
    let frontend = DummyArkFrontendNotebook::lock();

    // Allow stdin, so that a "Hit <Return>" prompt would block on an input
    // request instead of failing silently
    let options = ExecuteRequestOptions {
        allow_stdin: true,
        ..Default::default()
    };

    let code = "plot(lm(dist ~ speed, data = cars))";
    frontend.send_execute_request(code, options);
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    // One plot per diagnostic page, with no prompt in between
    let ids: Vec<String> = (0..4)
        .map(|_| frontend.recv_iopub_display_data_id())
        .collect();
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), 4);

    // The last page is completed after its first snapshot
    frontend.recv_iopub_update_display_data();

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}

#[test]
fn test_notebook_plot_layers_kept_when_next_page_starts_in_same_request() {
    let frontend = DummyArkFrontendNotebook::lock();

    // `plot.default()` holds the device while it calls `plot.new()`, so the
    // last layers of page 1 (the red point) are recorded while rendering is
    // held. They must still reach the frontend as an update for page 1, not
    // be dropped when page 2 starts (posit-dev/positron#17240).
    let code = "local({
        plot(1:10)
        points(5, 5, col = 'red')
        plot(1:5)
        points(3, 3, col = 'blue')
    })";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    let input = frontend.recv_iopub_execute_input();
    assert_eq!(input.code, code);

    let page_1 = frontend.recv_iopub_display_data_content();
    let page_2 = frontend.recv_iopub_display_data_content();
    let page_1_id = &page_1.transient["display_id"];
    let page_2_id = &page_2.transient["display_id"];
    assert_ne!(page_1_id, page_2_id);

    // Page 1 gets its held update once `plot.default()` releases the hold.
    // The image differs from the first snapshot because it has the red point.
    let page_1_update = frontend.recv_iopub_update_display_data_content();
    assert_eq!(page_1_update.transient.display_id, *page_1_id);
    assert_ne!(page_1_update.data["image/png"], page_1.data["image/png"]);

    // Page 2 gets its update when the request completes
    let page_2_update = frontend.recv_iopub_update_display_data_content();
    assert_eq!(page_2_update.transient.display_id, *page_2_id);

    frontend.recv_iopub_idle();
    assert_eq!(frontend.recv_shell_execute_reply(), input.execution_count);
}
