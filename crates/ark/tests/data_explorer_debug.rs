//
// data_explorer_debug.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

/// When selecting a different stack frame via DAP, the Data Explorer should NOT
/// receive any spurious events (unlike the Variables pane which refreshes).
#[test]
fn test_data_explorer_stable_on_frame_selection() {
    let frontend = DummyArkFrontend::lock();

    // Create a data frame and open it in the viewer
    frontend.execute_request_invisibly("test_df <- data.frame(a = 1:3)");
    let _comm_id = frontend.open_data_explorer("test_df");

    // Set up debug stack
    frontend.execute_request_invisibly("outer <- function() { outer_var <- 1; inner() }");
    frontend.execute_request_invisibly("inner <- function() { inner_var <- 2; browser() }");

    let mut dap = frontend.start_dap();

    frontend.send_execute_request("outer()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_start_debug();
    dap.recv_stopped();
    frontend.assert_stream_stdout_contains("Called from:");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Select a different frame
    let stack = dap.stack_trace();
    let outer_frame_id = stack[1].id;
    dap.evaluate(".positron_selected_frame", Some(outer_frame_id));

    // Data explorer should NOT have received any events
    frontend.assert_no_data_explorer_events();

    // Clean exit
    frontend.debug_send_quit();
    dap.recv_continued();

    // Clean up - removing test_df causes the data explorer to close
    frontend.send_execute_request(
        "rm(test_df, outer, inner)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // The data explorer closes when its binding is removed
    frontend.recv_iopub_comm_close();
}
