//
// kernel_notebook_object_explorer.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkPositronNotebook;

const OBJECT_EXPLORER_MIME: &str = "application/vnd.positron.objectExplorer+json";

#[test]
fn test_notebook_inline_object_explorer() {
    let frontend = DummyArkPositronNotebook::lock();
    let ui_comm_id = frontend.open_ui_comm();

    frontend.send_execute_request(
        "list(a = 1, b = list(c = 2))",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let comm_open = frontend.recv_iopub_comm_open();
    assert_eq!(comm_open.target_name, "positron.objectExplorer");
    assert_eq!(comm_open.data["inline_only"], true);

    let result_data = frontend.recv_iopub_execute_result_data();
    assert!(result_data.contains_key("text/plain"));
    let payload = result_data.get(OBJECT_EXPLORER_MIME).unwrap();
    assert_eq!(payload["version"], 1);
    assert_eq!(payload["title"], "list");
    assert_eq!(payload["comm_id"].as_str().unwrap(), comm_open.comm_id);

    frontend.recv_ui_prompt_state(&ui_comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_notebook_no_inline_object_explorer_for_atomic_vector() {
    let frontend = DummyArkPositronNotebook::lock();
    let ui_comm_id = frontend.open_ui_comm();

    frontend.send_execute_request("c(a = 1, b = 2)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let result_data = frontend.recv_iopub_execute_result_data();
    assert!(result_data.contains_key("text/plain"));
    assert!(!result_data.contains_key(OBJECT_EXPLORER_MIME));

    frontend.recv_ui_prompt_state(&ui_comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}
