//
// data_explorer_priority.rs
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
/// 2. Spawn 5 idle tasks that each sleep for 200ms.
/// 3. After the execute request completes, send `get_schema` through Shell.
///    Shell dispatches a `KernelRequest` to the R thread and blocks until
///    it's processed, so the ordering on IOPub is deterministic:
///    Busy -> CommMsg -> Idle.
/// 4. With the priority fix, R processes the kernel request after finishing
///    at most one idle task (~200ms). Without the fix, `select` picks
///    randomly among ready channels, so multiple idle tasks could run
///    first (~600ms+).
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

    // Spawn 5 idle tasks that each block the R thread for 200ms.
    // After this execute request completes, R enters the event loop with
    // all 5 tasks ready on the idle-task channel.
    frontend.send_execute_request(
        r#"invisible(.Call("ps_test_spawn_sleeping_idle_tasks", 5L, 200L))"#,
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
    // R is likely already executing an idle task (sleeping for 200ms).
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
    // one sleeping idle task (~200ms). Without the fix, `select` picks
    // randomly among ready channels, so multiple 200ms sleepers could
    // run first (~600ms+). The 350ms threshold is 200ms for one idle
    // task plus 150ms headroom for slow CI machines.
    assert!(
        schema_latency < Duration::from_millis(350),
        "get_schema took {schema_latency:?}, which suggests kernel requests \
         are being starved by idle tasks"
    );
}
