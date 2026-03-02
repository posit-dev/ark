//
// variables_debug.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

/// When entering debug mode (browser), the variables pane should switch to
/// showing the debug environment's bindings. When exiting, it should switch
/// back to the global environment.
#[test]
fn test_variables_pane_shows_debug_env() {
    let frontend = DummyArkFrontend::lock();

    // Set up a global variable before opening the variables comm
    frontend.execute_request_invisibly("test_gv <- 'hello'");

    // Open the variables comm and receive the initial Refresh
    let initial = frontend.open_variables_comm();
    let names: Vec<&str> = initial
        .variables
        .iter()
        .map(|v| v.display_name.as_str())
        .collect();
    assert!(names.contains(&"test_gv"));

    // Enter browser with a local variable
    frontend.send_execute_request(
        "local({ debug_var <- 42; browser() })",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stdout_contains("Called from:");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // The variables pane should have sent a Refresh with the debug env
    let refresh = frontend.recv_variables_refresh();
    let names: Vec<&str> = refresh
        .variables
        .iter()
        .map(|v| v.display_name.as_str())
        .collect();
    assert_eq!(names, vec!["debug_var"]);

    // Quit the debugger
    frontend.send_execute_request("Q", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Should be back to showing the global environment
    let refresh = frontend.recv_variables_refresh();
    let names: Vec<&str> = refresh
        .variables
        .iter()
        .map(|v| v.display_name.as_str())
        .collect();
    assert!(names.contains(&"test_gv"));
    assert!(!names.contains(&"debug_var"));

    // Clean up
    frontend.send_execute_request("rm(test_gv)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Consume the Update triggered by the rm()
    let _update = frontend.recv_variables_update();
}

/// When entering debug via a function call, the variables pane should show
/// the function's environment with its arguments.
#[test]
fn test_variables_pane_shows_function_debug_env() {
    let frontend = DummyArkFrontend::lock();

    // Set up a global variable before opening the variables comm
    frontend.execute_request_invisibly("test_gv2 <- 'world'");

    let _initial = frontend.open_variables_comm();

    // Define a function and call it so browser() triggers inside
    frontend.send_execute_request(
        "f <- function(my_arg) { browser(); my_arg }; f(99)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stdout_contains("Called from: f(99)");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // The debug env should show `my_arg`
    let refresh = frontend.recv_variables_refresh();
    let names: Vec<&str> = refresh
        .variables
        .iter()
        .map(|v| v.display_name.as_str())
        .collect();
    assert_eq!(names, vec!["my_arg"]);

    // Quit the debugger
    frontend.send_execute_request("Q", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Should be back to showing the global environment
    let refresh = frontend.recv_variables_refresh();
    let names: Vec<&str> = refresh
        .variables
        .iter()
        .map(|v| v.display_name.as_str())
        .collect();
    assert!(names.contains(&"test_gv2"));
    assert!(!names.contains(&"my_arg"));

    // Clean up
    frontend.send_execute_request("rm(test_gv2, f)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Consume the Update triggered by the rm()
    let _update = frontend.recv_variables_update();
}

/// When selecting a different stack frame via DAP, the variables pane should
/// refresh to show that frame's environment.
#[test]
fn test_variables_pane_refreshes_on_frame_selection() {
    let frontend = DummyArkFrontend::lock();

    // Define functions before opening comms
    frontend
        .execute_request_invisibly("outer <- function() { outer_var <- 'from_outer'; inner() }");
    frontend
        .execute_request_invisibly("inner <- function() { inner_var <- 'from_inner'; browser() }");

    let _initial = frontend.open_variables_comm();
    let mut dap = frontend.start_dap();

    // Enter debug mode
    frontend.send_execute_request("outer()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_start_debug();
    dap.recv_stopped();

    // Variables pane shows the inner frame (current debug frame)
    let refresh = frontend.recv_variables_refresh();
    let names: Vec<&str> = refresh
        .variables
        .iter()
        .map(|v| v.display_name.as_str())
        .collect();
    assert_eq!(names, vec!["inner_var"]);

    frontend.assert_stream_stdout_contains("Called from:");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Get stack to find frame IDs
    let stack = dap.stack_trace();
    assert!(stack.len() >= 2);
    let outer_frame_id = stack[1].id;

    // Select the outer frame via DAP
    dap.evaluate(".positron_selected_frame", Some(outer_frame_id));

    // Variables pane should refresh to show the outer frame's variables
    let refresh = frontend.recv_variables_refresh();
    let names: Vec<&str> = refresh
        .variables
        .iter()
        .map(|v| v.display_name.as_str())
        .collect();
    assert_eq!(names, vec!["outer_var"]);

    // Clean exit
    frontend.debug_send_quit();
    dap.recv_continued();

    // Consume the refresh back to global env
    let _refresh = frontend.recv_variables_refresh();

    // Clean up
    frontend.send_execute_request("rm(outer, inner)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Consume the Update triggered by the rm()
    let _update = frontend.recv_variables_update();
}
