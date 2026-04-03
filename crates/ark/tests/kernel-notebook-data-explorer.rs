//
// kernel-notebook-data-explorer.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkPositronNotebook;

/// Drain the UI comm messages that arrive during execution (busy=true,
/// busy=false, prompt_state). These are CommMsg messages on the UI comm's
/// channel that interleave with the execute result on IOPub.
fn drain_ui_comm_msgs(frontend: &DummyArkPositronNotebook, ui_comm_id: &str) {
    // busy=true
    let msg = frontend.recv_iopub_comm_msg();
    assert_eq!(msg.comm_id, ui_comm_id);
    assert_eq!(msg.data["method"], "busy");
    assert_eq!(msg.data["params"]["busy"], true);

    // busy=false
    let msg = frontend.recv_iopub_comm_msg();
    assert_eq!(msg.comm_id, ui_comm_id);
    assert_eq!(msg.data["method"], "busy");
    assert_eq!(msg.data["params"]["busy"], false);
}

fn drain_ui_comm_prompt_state(frontend: &DummyArkPositronNotebook, ui_comm_id: &str) {
    let msg = frontend.recv_iopub_comm_msg();
    assert_eq!(msg.comm_id, ui_comm_id);
    assert_eq!(msg.data["method"], "prompt_state");
}

#[test]
fn test_notebook_inline_data_explorer() {
    let frontend = DummyArkPositronNotebook::lock();
    let ui_comm_id = frontend.open_ui_comm();

    frontend.send_execute_request(
        "data.frame(x = 1:3, y = 4:6)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Drain UI comm busy events
    drain_ui_comm_msgs(&frontend, &ui_comm_id);

    let result_data = frontend.recv_iopub_execute_result_data();

    // Should have text/plain (autoprint output)
    assert!(result_data.contains_key("text/plain"));
    assert!(!result_data.contains_key("text/html"));

    // Should have the inline data explorer MIME type
    let mime_key = "application/vnd.positron.dataExplorer+json";
    assert!(result_data.contains_key(mime_key));

    let de_data = result_data.get(mime_key).unwrap();
    assert_eq!(de_data["version"], 1);
    assert_eq!(de_data["shape"]["rows"], 3);
    assert_eq!(de_data["shape"]["columns"], 2);
    assert!(de_data["comm_id"].as_str().is_some());
    assert!(de_data["title"].as_str().is_some());

    // prompt_state arrives after execute_result
    drain_ui_comm_prompt_state(&frontend, &ui_comm_id);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // The comm_open for the inline data explorer arrives after Idle
    // (it goes through Shell's comm event channel)
    let comm_open = frontend.recv_iopub_comm_open();
    assert_eq!(comm_open.target_name, "positron.dataExplorer");
    assert_eq!(comm_open.data["inline_only"], true);
    assert_eq!(comm_open.comm_id, de_data["comm_id"].as_str().unwrap());
}

#[test]
fn test_notebook_no_inline_data_explorer_for_non_data_frame() {
    let frontend = DummyArkPositronNotebook::lock();
    let ui_comm_id = frontend.open_ui_comm();

    frontend.send_execute_request("1:10", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    drain_ui_comm_msgs(&frontend, &ui_comm_id);

    let result_data = frontend.recv_iopub_execute_result_data();

    // Should have text/plain but NOT the data explorer MIME type
    assert!(result_data.contains_key("text/plain"));
    assert!(!result_data.contains_key("application/vnd.positron.dataExplorer+json"));

    drain_ui_comm_prompt_state(&frontend, &ui_comm_id);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}
