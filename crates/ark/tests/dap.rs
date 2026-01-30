//
// dap.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::io::Write;

use ark_test::assert_file_frame;
use ark_test::assert_vdoc_frame;
use ark_test::DummyArkFrontend;
use dap::types::Thread;

#[test]
fn test_dap_initialize_and_disconnect() {
    let frontend = DummyArkFrontend::lock();

    // `start_dap()` connects and initializes, `Drop` disconnects
    let mut dap = frontend.start_dap();

    // First thing sent by frontend after connection
    assert!(matches!(
        dap.threads().as_slice(),
        [Thread { id: -1, name }] if name == "R console"
    ));
}

#[test]
fn test_dap_stopped_at_browser() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.debug_send_browser();

    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 1);

    // line: 1, column: 10 corrsponds to `browser()`
    assert_vdoc_frame(&stack[0], "<global>", 1, 10);

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_source_and_step() {
    use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
    use amalthea::wire::jupyter_message::Message;
    use amalthea::wire::status::ExecutionState;

    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let mut file = tempfile::NamedTempFile::new().unwrap();
    // Use a braced block so `n` can step within the sourced expression.
    write!(file, "1\n2\n{{\nbrowser()\n3\n4\n}}\n").unwrap();
    let path = file.path().to_str().unwrap().replace("\\", "/");
    let filename = file.path().file_name().unwrap().to_str().unwrap();

    // Source the file - it will stop at browser()
    frontend.send_execute_request(
        &format!("source('{path}')"),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    frontend.recv_iopub_all(vec![
        Box::new(|msg| {
            matches!(
                msg,
                Message::CommMsg(comm) if comm.content.data["method"] == "start_debug"
            )
        }),
        Box::new(|msg| {
            let Message::Stream(stream) = msg else {
                return false;
            };
            stream.content.text.contains("Called from:")
        }),
        Box::new(|msg| {
            matches!(
                msg,
                Message::Status(s) if s.content.execution_state == ExecutionState::Idle
            )
        }),
    ]);

    frontend.recv_shell_execute_reply();
    dap.recv_stopped();

    // Check stack at browser() - line 4, end_column 10 for `browser()`
    let stack = dap.stack_trace();
    assert!(stack.len() >= 1, "Expected at least 1 frame");
    assert_file_frame(&stack[0], filename, 4, 10);

    frontend.debug_send_step_command("n");
    dap.recv_continued();
    dap.recv_stopped();

    // After stepping, we should be at line 5 (the `3` expression after browser())
    let stack = dap.stack_trace();
    assert!(stack.len() >= 1, "Expected at least 1 frame after step");
    assert_file_frame(&stack[0], filename, 5, 2);

    // Exit with Q via Jupyter
    frontend.debug_send_quit();
    dap.recv_continued();
}
