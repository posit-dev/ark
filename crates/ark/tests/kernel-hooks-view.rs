//
// kernel-hooks-view.rs
//
// Copyright (C) 2026 by Posit Software, PBC
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

/// `View()` on a namespace function must materialise the virtual namespace
/// document at `ark:.../namespace/<pkg>.R` and emit an `open_editor` UI event
/// pointing at it.
#[test]
fn test_view_namespace_function_generates_vdoc() {
    let frontend = DummyArkFrontend::lock();
    let comm_id = frontend.open_ui_comm();

    frontend.send_execute_request("View(identity)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // `view_function()` ends with `.ps.ui.navigateToFile()`, emitted on the UI
    // comm before the surrounding `busy(true/false)` / `prompt_state` pair.
    let open_editor = frontend.recv_iopub_comm_msg();
    assert_eq!(open_editor.comm_id, comm_id);
    assert_eq!(
        open_editor.data.get("method").and_then(|v| v.as_str()),
        Some("open_editor")
    );

    // Test runs in the kernel's own process, so `std::process::id()` matches
    // the pid that `ark_uri()` embeds.
    let pid = std::process::id();
    let expected_uri = format!("ark:ark-{pid}/namespace/base.R");

    let params = &open_editor.data["params"];
    assert_eq!(params["file"], expected_uri);
    assert_eq!(params["kind"], "uri");
    // `identity`'s srcref points somewhere inside the generated namespace
    // file. We only care that it's a real position, not the start.
    assert!(params["line"].as_i64().unwrap() > 0);

    frontend.recv_ui_prompt_state(&comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}
