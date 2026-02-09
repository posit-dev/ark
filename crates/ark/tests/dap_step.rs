//
// dap_step.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use ark_test::assert_file_frame;
use ark_test::DummyArkFrontend;

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

    frontend.debug_send_step_command("n", &file);
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

#[test]
fn test_dap_step_into_function() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = frontend.send_source(
        "
foo <- function() {
  browser()
  bar()
  2
}
bar <- function() {
  1
}
foo()
",
    );
    dap.recv_stopped();

    // Check initial stack at browser() in foo
    dap.assert_top_frame("foo()");
    let stack = dap.stack_trace();
    assert_file_frame(&stack[0], &file.filename, 3, 12);

    // Step with `n` to the bar() call
    frontend.debug_send_step_command("n", &file);
    dap.recv_continued();
    dap.recv_stopped();

    dap.assert_top_frame("foo()");
    let stack = dap.stack_trace();
    assert_file_frame(&stack[0], &file.filename, 4, 8);

    // Step with `s` into bar()
    frontend.debug_send_step_command("s", &file);
    dap.recv_continued();
    dap.recv_stopped();

    // Verify stack has 2 frames: bar on top, foo below
    let stack = dap.stack_trace();
    assert!(stack.len() >= 2, "Expected at least 2 frames after step in");
    assert_eq!(stack[0].name, "bar()");
    assert_eq!(stack[1].name, "foo()");

    // Step out with `f` (finish)
    frontend.debug_send_step_command("f", &file);
    dap.recv_continued();
    dap.recv_stopped();

    // Verify we're back in foo
    dap.assert_top_frame("foo()");

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_continue() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = frontend.send_source(
        "
{
  browser()
  1
  browser()
  2
}
",
    );
    dap.recv_stopped();

    // Check we're at first browser()
    let stack = dap.stack_trace();
    assert_file_frame(&stack[0], &file.filename, 3, 12);

    // Continue with `c` to next browser()
    frontend.debug_send_continue_to_breakpoint();
    dap.recv_continued();
    dap.recv_stopped();

    // Verify we stopped at second browser()
    let stack = dap.stack_trace();
    assert_file_frame(&stack[0], &file.filename, 5, 12);

    // Continue again - should exit debug session
    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_step_out() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // outer() has browser() so we stay in debug mode after stepping out of inner()
    let file = frontend.send_source(
        "
outer <- function() {
  browser()
  inner()
  2
}
inner <- function() {
  x <- 1
  x
}
outer()
",
    );
    dap.recv_stopped();

    // Check initial stack at browser() in outer
    dap.assert_top_frame("outer()");
    let stack = dap.stack_trace();
    assert_file_frame(&stack[0], &file.filename, 3, 12);

    // Step with `n` to inner() call
    frontend.debug_send_step_command("n", &file);
    dap.recv_continued();
    dap.recv_stopped();

    // Step into inner() with `s`
    frontend.debug_send_step_command("s", &file);
    dap.recv_continued();
    dap.recv_stopped();

    // Verify we're in inner()
    let stack = dap.stack_trace();
    assert!(stack.len() >= 2, "Expected at least 2 frames after step in");
    assert_eq!(stack[0].name, "inner()");
    assert_eq!(stack[1].name, "outer()");

    // Step out with `f` (finish)
    frontend.debug_send_step_command("f", &file);
    dap.recv_continued();
    dap.recv_stopped();

    // Verify we're back in outer, at the line after inner() call
    dap.assert_top_frame("outer()");

    frontend.debug_send_quit();
    dap.recv_continued();
}
