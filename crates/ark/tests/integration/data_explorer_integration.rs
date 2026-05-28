//
// data_explorer_integration.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
// Integration test verifying that Shell requests (kernel requests) are
// prioritized over idle tasks in the event loop. This reproduces the
// scenario from the data explorer performance test failure where
// `get_schema` timed out because the R thread was busy servicing
// column profile idle tasks instead of picking up the kernel request.

use std::time::Duration;
use std::time::Instant;

use amalthea::comm::data_explorer_comm::DataExplorerBackendReply;
use amalthea::comm::data_explorer_comm::DataExplorerBackendRequest;
use amalthea::comm::data_explorer_comm::GetSchemaParams;
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

/// Verify that Shell requests (`get_schema`) are prioritized over idle tasks.
///
/// 1. Open a data explorer so we have a comm to send RPCs on.
/// 2. Spawn 5 idle tasks that each sleep for 500ms.
/// 3. After the execute request completes, send `get_schema` through Shell.
///    Shell dispatches a `KernelRequest` to the R thread and blocks until
///    it's processed, so the ordering on IOPub is deterministic:
///    Busy -> CommMsg -> Idle.
/// 4. With the priority fix, R processes the kernel request after finishing
///    at most one idle task (~500ms). Without the fix, `select` picks
///    randomly among ready channels, so multiple idle tasks could run
///    first (~1500ms+).
#[test]
fn test_kernel_request_priority_over_idle_tasks() {
    let frontend = DummyArkFrontend::lock();

    // A small data frame is enough -- the contention comes from the
    // sleeping idle tasks, not from profile computation.
    frontend.send_execute_request(
        "test_priority_df <- data.frame(a = 1:5)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    let comm_id = frontend.open_data_explorer("test_priority_df");

    // Build the get_schema RPC data. The `id` field marks it as an RPC
    // so Shell creates a `CommMsg::Rpc`.
    let schema_request = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
        column_indices: vec![0],
    });
    let mut data = serde_json::to_value(&schema_request).unwrap();
    data["id"] = serde_json::Value::String(String::from("test-rpc"));

    // Spawn 5 idle tasks that each block the R thread for 500ms.
    // After this execute request completes, R enters the event loop with
    // all 5 tasks ready on the idle-task channel.
    frontend.send_execute_request(
        r#"invisible(.Call("ps_test_spawn_sleeping_idle_tasks", 5L, 500L))"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Send get_schema through Shell. Shell dispatches a `KernelRequest`
    // to the R thread and blocks on `done_rx`, so the IOPub ordering is
    // deterministic: Busy -> CommMsg -> Idle (no race).
    //
    // R is likely already executing an idle task (sleeping for 500ms).
    // When it finishes, the priority check at the top of the event loop
    // picks up the kernel request before `select` can hand R another
    // idle task.
    let start = Instant::now();
    frontend.send_shell_comm_msg(comm_id.clone(), data);

    frontend.recv_iopub_busy();
    let msg = frontend.recv_iopub_comm_msg();
    let schema_latency = start.elapsed();
    frontend.recv_iopub_idle();

    assert_eq!(msg.comm_id, comm_id);
    let reply: DataExplorerBackendReply = serde_json::from_value(msg.data).unwrap();

    match reply {
        DataExplorerBackendReply::GetSchemaReply(schema) => {
            assert_eq!(schema.columns.len(), 1);
        },
        other => panic!("Expected GetSchemaReply, got: {other:?}"),
    }

    // With the priority fix the kernel request is serviced after at most
    // one sleeping idle task (~500ms). Without the fix, `select` picks
    // randomly among ready channels, so multiple 500ms sleepers could
    // run first (~1500ms+). The 750ms threshold is 500ms for one idle
    // task plus 250ms headroom for slow CI machines.
    assert!(
        schema_latency < Duration::from_millis(750),
        "get_schema took {schema_latency:?}, which suggests kernel requests \
         are being starved by idle tasks"
    );
}

/// The `OpenDataExplorer` RPC calls `comm_open_backend` from inside the
/// handler to open a child explorer. This inserts into the `comms`
/// HashMap while the parent comm has been taken out for dispatch.
/// Without the take pattern, this would panic on a reentrant
/// `borrow_mut()`.
#[test]
fn test_open_child_explorer_during_dispatch() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_execute_request(
        "test_df <- data.frame(x = 1:3, y = letters[1:3])",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    let parent_comm_id = frontend.open_data_explorer("test_df");

    // Send the OpenDataExplorer RPC to the parent explorer.
    let request = DataExplorerBackendRequest::OpenDataExplorer;
    let mut data = serde_json::to_value(&request).unwrap();
    data["id"] = serde_json::Value::String(String::from("open-rpc"));

    frontend.send_shell_comm_msg(parent_comm_id.clone(), data);
    frontend.recv_iopub_busy();

    // The handler calls `comm_open_backend`, which opens a new data
    // explorer comm. Shell drains comm events during the handler, so
    // the child's `comm_open` arrives before the RPC reply.
    let child_open = frontend.recv_iopub_comm_open();
    assert_eq!(child_open.target_name, "positron.dataExplorer");
    assert_ne!(child_open.comm_id, parent_comm_id);

    // The RPC reply follows.
    let reply_msg = frontend.recv_iopub_comm_msg();
    assert_eq!(reply_msg.comm_id, parent_comm_id);
    let reply: DataExplorerBackendReply = serde_json::from_value(reply_msg.data).unwrap();
    assert_eq!(reply, DataExplorerBackendReply::OpenDataExplorerReply());

    frontend.recv_iopub_idle();
}
