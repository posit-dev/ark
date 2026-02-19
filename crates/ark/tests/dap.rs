//
// dap.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
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

    // Execute an expression that doesn't advance the debugger.
    // Transient evals send Invalidated instead of Continued+Stopped.
    frontend.debug_send_expr("1");
    dap.recv_invalidated();

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_nested_stack_frames() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = frontend.send_source(
        "
a <- function() { b() }
b <- function() { c() }
c <- function() { browser() }
a()
",
    );
    dap.recv_stopped();

    // Check stack at browser() in c - should have 3 frames: c, b, a
    let stack = dap.stack_trace();
    assert!(
        stack.len() >= 3,
        "Expected at least 3 frames, got {}",
        stack.len()
    );

    // Verify frame names (innermost to outermost)
    assert_file_frame(&stack[0], &file.filename, 4, 28);
    assert_eq!(stack[0].name, "c()");

    assert_file_frame(&stack[1], &file.filename, 3, 22);
    assert_eq!(stack[1].name, "b()");

    assert_file_frame(&stack[2], &file.filename, 2, 22);
    assert_eq!(stack[2].name, "a()");

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Test that browser() inside dplyr::mutate() doesn't crash.
///
/// This is a regression test for https://github.com/posit-dev/positron/issues/8979
/// where R_Srcref could be a NULL pointer in dplyr's data mask context, causing
/// a SIGSEGV when passed to R functions.
#[test]
fn test_dap_browser_in_dplyr_mutate() {
    let frontend = DummyArkFrontend::lock();

    if !frontend.is_installed("dplyr") {
        println!("Skipping test: dplyr package not installed");
        return;
    }

    let mut dap = frontend.start_dap();

    frontend.send_execute_request(
        "mtcars |> dplyr::mutate(browser())",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_start_debug();

    // Should stop at browser() without crashing
    dap.recv_stopped();

    // Verify we have a valid stack frame
    let stack = dap.stack_trace();
    assert!(!stack.is_empty(), "Expected at least one stack frame");

    // The execute_request completes after browser() is entered, before we quit
    frontend.assert_stream_stdout_contains("Called from:");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Quit the debugger
    frontend.send_execute_request("Q", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    dap.recv_continued();
}

#[test]
fn test_dap_recursive_function() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Recursive function that hits browser() at the base case
    let _file = frontend.send_source(
        "
factorial <- function(n) {
  if (n <= 1) {
    browser()
    return(1)
  }
  n * factorial(n - 1)
}
factorial(3)
",
    );
    dap.recv_stopped();

    // Should be at browser() when n=1, with multiple factorial() frames
    let stack = dap.stack_trace();

    // Count how many factorial() frames we have
    let factorial_frames: Vec<_> = stack.iter().filter(|f| f.name == "factorial()").collect();
    assert!(
        factorial_frames.len() >= 3,
        "Should have at least 3 factorial() frames for factorial(3), got {}",
        factorial_frames.len()
    );

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_stack_trace_total_frames() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Recursive function that creates a deep call stack
    let _file = frontend.send_source(
        "
recurse <- function(n) {
  if (n <= 1) {
    browser()
    return(1)
  }
  recurse(n - 1)
}
recurse(10)
",
    );
    dap.recv_stopped();

    // Get the full stack to know the total size
    let full_stack = dap.stack_trace();
    let total = full_stack.len() as i64;
    assert!(total >= 10);

    // Request only the first 3 frames. `total_frames` must still
    // report the full stack size so the frontend knows to page.
    let page = dap.stack_trace_paged(0, 3);
    assert_eq!(page.stack_frames.len(), 3);
    assert_eq!(page.total_frames, Some(total));

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_error_during_debug() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Code that will error after browser()
    let file = frontend.send_source(
        "
{
  browser()
  stop('intentional error')
}
",
    );
    dap.recv_stopped();

    // We're at browser(), stack should have 1 frame
    let stack = dap.stack_trace();
    assert!(stack.len() >= 1, "Should have at least 1 frame");

    // Step to execute the error
    frontend.debug_send_step_command("n", &file);
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_error_in_eval() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Enter debug mode via browser() in virtual doc context
    frontend.debug_send_browser();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 1, "Should have 1 frame");

    // Evaluate an expression that causes an error.
    // Unlike stepping to an error (which exits debug), evaluating an error
    // from the console should keep us in debug mode.
    // Transient evals send Invalidated instead of Continued+Stopped.
    frontend.send_execute_request("stop('eval error')", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    dap.recv_invalidated();

    let evalue = frontend.recv_iopub_execute_error();
    assert!(evalue.contains("eval error"));
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();
}

#[test]
fn test_dap_nested_browser() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Enter debug mode via browser() in virtual doc context
    frontend.debug_send_browser();
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 1, "Should have 1 frame at Browse[1]>");

    // Enter nested debug by calling a function with debugonce
    frontend.send_execute_request(
        "debugonce(identity); identity(1)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    frontend.recv_iopub_stop_debug();
    frontend.assert_stream_stdout_contains("debugging in:");
    frontend.recv_iopub_start_debug();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // DAP: Continued (exiting Browse[1]>), then Stopped (entering Browse[2]>)
    dap.recv_continued();
    dap.recv_stopped();

    // Stack now shows 2 frames: identity() and the original browser frame
    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 2, "Should have 2 frames at Browse[2]>");
    assert_eq!(stack[0].name, "identity()");

    // Step with `n` to return to parent browser (Browse[1]>)
    frontend.send_execute_request("n", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();
    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // DAP: Continued (left identity) then Stopped (back at parent browser)
    dap.recv_continued();
    dap.recv_stopped();

    // Back to 1 frame at Browse[1]>
    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 1, "Should have 1 frame back at Browse[1]>");

    // Now quit entirely
    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_hidden_frames_filtered() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Simulate Shiny's stack trace filtering with the sentinel functions.
    // Frames between `..stacktraceon..` and `..stacktraceoff..` should be hidden.
    let _file = frontend.send_source(
        "
`..stacktraceoff..` <- function(x) x
`..stacktraceon..` <- function(x) x

user_code <- function() { browser() }
shiny_internal <- function() { `..stacktraceon..`(user_code()) }
shiny_wrapper <- function() { `..stacktraceoff..`(shiny_internal()) }
outer_user <- function() { shiny_wrapper() }

outer_user()
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert!(stack.len() >= 3);
    assert_eq!(stack[0].name, "user_code()");
    assert_eq!(stack[1].name, "shiny_wrapper()");
    assert_eq!(stack[2].name, "outer_user()");

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_hidden_frames_show_with_option() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Enable the option to show hidden frames
    frontend.send_execute_request(
        "options(ark.debugger.show_hidden_frames = TRUE)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Same code as above with sentinel functions
    let _file = frontend.send_source(
        "
`..stacktraceoff..` <- function(x) x
`..stacktraceon..` <- function(x) x

user_code <- function() { browser() }
shiny_internal <- function() { `..stacktraceon..`(user_code()) }
shiny_wrapper <- function() { `..stacktraceoff..`(shiny_internal()) }
outer_user <- function() { shiny_wrapper() }

outer_user()
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();

    frontend.debug_send_quit();
    dap.recv_continued();

    // Clean up option before assertions so it doesn't leak on failure
    frontend.send_execute_request(
        "options(ark.debugger.show_hidden_frames = NULL)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    assert!(stack.len() >= 6);
    assert_eq!(stack[0].name, "user_code()");
    assert_eq!(stack[1].name, "..stacktraceon..()");
    assert_eq!(stack[2].name, "shiny_internal()");
    assert_eq!(stack[3].name, "..stacktraceoff..()");
    assert_eq!(stack[4].name, "shiny_wrapper()");
    assert_eq!(stack[5].name, "outer_user()");
}

#[test]
fn test_dap_hidden_frames_sequential_regions() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Two sequential hidden regions separated by visible user code
    let _file = frontend.send_source(
        "
`..stacktraceoff..` <- function(x) x
`..stacktraceon..` <- function(x) x

user_code <- function() { browser() }
inner_on <- function() { `..stacktraceon..`(user_code()) }
internal_a <- function() { inner_on() }
inner_off <- function() { `..stacktraceoff..`(internal_a()) }
middle_user <- function() { inner_off() }
outer_on <- function() { `..stacktraceon..`(middle_user()) }
internal_b <- function() { outer_on() }
outer_off <- function() { `..stacktraceoff..`(internal_b()) }
top <- function() { outer_off() }

top()
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert!(stack.len() >= 5);
    assert_eq!(stack[0].name, "user_code()");
    assert_eq!(stack[1].name, "inner_off()");
    assert_eq!(stack[2].name, "middle_user()");
    assert_eq!(stack[3].name, "outer_off()");
    assert_eq!(stack[4].name, "top()");

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_hidden_frames_topmost_preserved() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // If `..stacktraceoff..` is missing, the topmost frame (where the user
    // is stopped) must still be visible so they can see where they are.
    let _file = frontend.send_source(
        "
`..stacktraceon..` <- function(x) x

user_code <- function() { browser() }
wrapper <- function() { `..stacktraceon..`(user_code()) }

wrapper()
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    assert_eq!(stack.len(), 1);
    assert_eq!(stack[0].name, "user_code()");

    frontend.debug_send_quit();
    dap.recv_continued();
}

#[test]
fn test_dap_show_hidden_frames_fenced() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.send_execute_request(
        "options(ark.debugger.show_hidden_frames = 'fenced')",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    let _file = frontend.send_source(
        "
`..stacktraceoff..` <- function(x) x
`..stacktraceon..` <- function(x) x

user_code <- function() { browser() }
shiny_internal <- function() { `..stacktraceon..`(user_code()) }
shiny_wrapper <- function() { `..stacktraceoff..`(shiny_internal()) }
outer_user <- function() { shiny_wrapper() }

outer_user()
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();

    frontend.debug_send_quit();
    dap.recv_continued();

    frontend.send_execute_request(
        "options(ark.debugger.show_hidden_frames = NULL)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // With "fenced", sentinel frames should be visible
    assert!(stack.len() >= 6);
    assert_eq!(stack[0].name, "user_code()");
    assert_eq!(stack[1].name, "..stacktraceon..()");
    assert_eq!(stack[2].name, "shiny_internal()");
    assert_eq!(stack[3].name, "..stacktraceoff..()");
    assert_eq!(stack[4].name, "shiny_wrapper()");
    assert_eq!(stack[5].name, "outer_user()");
}

#[test]
fn test_dap_show_hidden_frames_fenced_still_hides_internal() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.send_execute_request(
        "options(ark.debugger.show_hidden_frames = 'fenced')",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.set_exception_breakpoints(&["error"]);

    frontend.send_source("stop('test')");

    dap.recv_stopped_exception();

    let stack = dap.stack_trace();
    let frame_names: Vec<&str> = stack.iter().map(|f| f.name.as_str()).collect();

    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_execute_error();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();

    dap.recv_continued();

    frontend.send_execute_request(
        "options(ark.debugger.show_hidden_frames = NULL)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // With "fenced", internal condition-handling frames should still be hidden
    assert!(!frame_names
        .iter()
        .any(|f| f.starts_with(".handleSimpleError")));
}

#[test]
fn test_dap_show_hidden_frames_internal() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.send_execute_request(
        "options(ark.debugger.show_hidden_frames = 'internal')",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.set_exception_breakpoints(&["error"]);

    frontend.send_source("stop('test')");

    dap.recv_stopped_exception();

    let stack = dap.stack_trace();
    let frame_names: Vec<&str> = stack.iter().map(|f| f.name.as_str()).collect();

    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_execute_error();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();

    dap.recv_continued();

    frontend.send_execute_request(
        "options(ark.debugger.show_hidden_frames = NULL)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // With "internal", the .handleSimpleError() frame should now be visible
    assert!(frame_names
        .iter()
        .any(|f| f.starts_with(".handleSimpleError")));
}

#[test]
fn test_dap_show_hidden_frames_internal_still_hides_fenced() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    frontend.send_execute_request(
        "options(ark.debugger.show_hidden_frames = 'internal')",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    let _file = frontend.send_source(
        "
`..stacktraceoff..` <- function(x) x
`..stacktraceon..` <- function(x) x

user_code <- function() { browser() }
shiny_internal <- function() { `..stacktraceon..`(user_code()) }
shiny_wrapper <- function() { `..stacktraceoff..`(shiny_internal()) }
outer_user <- function() { shiny_wrapper() }

outer_user()
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();

    frontend.debug_send_quit();
    dap.recv_continued();

    frontend.send_execute_request(
        "options(ark.debugger.show_hidden_frames = NULL)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // With "internal", fenced frames should still be hidden
    assert!(stack.len() >= 3);
    assert_eq!(stack[0].name, "user_code()");
    assert_eq!(stack[1].name, "shiny_wrapper()");
    assert_eq!(stack[2].name, "outer_user()");
}

/// https://github.com/posit-dev/positron/issues/11780
/// `browser()` inside `tryCatch()` must evaluate in the function's environment,
/// not a parent one.
#[test]
fn test_dap_browser_in_trycatch() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
f <- function(my_var) {
  tryCatch(
    {
      browser()
      my_var
    }
  )
}
f(1)
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let frame_id = stack[0].id;
    let scopes = dap.scopes(frame_id);
    let variables = dap.variables(scopes[0].variables_reference);

    let var = variables.iter().find(|v| v.name == "my_var").unwrap();
    assert_eq!(var.value, "1");

    // Evaluate `my_var` from the console: must resolve to the argument
    frontend.send_execute_request("my_var", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();

    frontend.recv_iopub_start_debug();
    frontend.assert_stream_stdout_contains("[1] 1");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Same as above but with `withCallingHandlers()`.
#[test]
fn test_dap_browser_in_withcallinghandlers() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
f <- function(my_var) {
  withCallingHandlers(
    {
      browser()
      my_var
    },
    warning = function(w) invokeRestart('muffleWarning')
  )
}
f(99)
",
    );
    dap.recv_stopped();

    let stack = dap.stack_trace();
    let frame_id = stack[0].id;
    let scopes = dap.scopes(frame_id);
    let variables = dap.variables(scopes[0].variables_reference);

    let var = variables.iter().find(|v| v.name == "my_var").unwrap();
    assert_eq!(var.value, "99");

    // Evaluate `my_var` from the console: must resolve to the argument
    frontend.send_execute_request("my_var", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();
    frontend.assert_stream_stdout_contains("[1] 99");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    dap.recv_continued();
    dap.recv_stopped();

    frontend.debug_send_quit();
    dap.recv_continued();
}
