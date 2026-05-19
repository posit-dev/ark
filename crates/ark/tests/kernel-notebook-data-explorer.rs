//
// kernel-notebook-data-explorer.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkPositronNotebook;

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

    // The comm_open for the inline data explorer now arrives during execution
    // (before the execute result) because comm_open_backend blocks until Shell
    // has published it on IOPub.
    let comm_open = frontend.recv_iopub_comm_open();
    assert_eq!(comm_open.target_name, "positron.dataExplorer");
    assert_eq!(comm_open.data["inline_only"], true);

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

    // The comm_id in the MIME payload must match the comm_open
    assert_eq!(comm_open.comm_id, de_data["comm_id"].as_str().unwrap());

    // prompt_state arrives after execute_result
    frontend.recv_ui_prompt_state(&ui_comm_id);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_notebook_no_inline_data_explorer_for_non_data_frame() {
    let frontend = DummyArkPositronNotebook::lock();
    let ui_comm_id = frontend.open_ui_comm();

    frontend.send_execute_request("1:10", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let result_data = frontend.recv_iopub_execute_result_data();

    // Should have text/plain but NOT the data explorer MIME type
    assert!(result_data.contains_key("text/plain"));
    assert!(!result_data.contains_key("application/vnd.positron.dataExplorer+json"));

    frontend.recv_ui_prompt_state(&ui_comm_id);

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}
