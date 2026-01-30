//
// dap.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

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

    // Execute an expression that doesn't advance the debugger
    // FIXME: `preserve_focus_hint` should be false
    // https://github.com/posit-dev/positron/issues/11604
    frontend.debug_send_expr("1");
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_source_and_step() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Use a braced block so `n` can step within the sourced expression.
    let file = frontend.send_source(
        "
1
2
{
  browser()
  3
  4
}
",
    );
    dap.recv_stopped();

    // Check stack at browser() - line 4, end_column 10 for `browser()`
    let stack = dap.stack_trace();
    assert!(stack.len() >= 1, "Expected at least 1 frame");
    assert_file_frame(&stack[0], &file.filename, 5, 12);

    frontend.debug_send_step_command("n");
    dap.recv_continued();
    dap.recv_stopped();

    // After stepping, we should be at line 5 (the `3` expression after browser())
    let stack = dap.stack_trace();
    assert!(stack.len() >= 1, "Expected at least 1 frame after step");
    assert_file_frame(&stack[0], &file.filename, 6, 4);

    // Exit with Q via Jupyter
    frontend.debug_send_quit();
    dap.recv_continued();
}
