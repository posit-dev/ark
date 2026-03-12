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
/// 2. In a single execute request, spawn 5 idle tasks that each sleep for
///    200ms **and** enqueue a `get_schema` kernel request directly on the
///    kernel-request channel. Because both happen inside the same execute
///    window, the kernel request is already pending when R returns to the
///    event loop — no race.
/// 3. Assert that the reply arrives in well under a single sleep duration.
///    With the priority fix the kernel request is serviced before any
///    sleeping idle task runs, so the reply is near-instant. Without the
///    fix `select` picks randomly, so at least one 200ms sleeper runs
///    first.
#[test]
fn test_kernel_request_priority_over_idle_tasks() {
    let frontend = DummyArkFrontend::lock();

    // A small data frame is enough — the contention comes from the
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

    // Build the JSON for the `get_schema` RPC.
    let schema_request = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
        column_indices: vec![0],
    });
    let schema_json = serde_json::to_string(&schema_request).unwrap();

    // In a single execute request:
    // - Spawn 5 idle tasks that each block the R thread for 200ms
    // - Enqueue a `get_schema` kernel request directly on the
    //   kernel-request channel via `ps_test_send_kernel_comm_request`
    //
    // Both are pending when R returns to the event loop.
    let code = format!(
        r#"
        invisible(.Call("ps_test_spawn_sleeping_idle_tasks", 5L, 200L))
        invisible(.Call("ps_test_send_kernel_comm_request", "{comm_id}", '{schema_json}'))
        "#,
    );
    frontend.send_execute_request(&code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // The kernel request was enqueued directly (bypassing Shell), so the
    // RPC reply arrives on IOPub without the Shell's Busy/Idle wrapper.
    let start = Instant::now();
    let msg = frontend.recv_iopub_comm_msg();
    let schema_latency = start.elapsed();

    assert_eq!(msg.comm_id, comm_id);
    let reply: DataExplorerBackendReply = serde_json::from_value(msg.data).unwrap();

    match reply {
        DataExplorerBackendReply::GetSchemaReply(schema) => {
            assert_eq!(schema.columns.len(), 1);
        },
        other => panic!("Expected GetSchemaReply, got: {other:?}"),
    }

    // With the priority fix the kernel request is serviced before any
    // sleeping idle task runs, so the reply is near-instant. Without the
    // fix, `select` picks randomly between the kernel-request channel and
    // the idle-task channel, so at least one 200ms sleeper runs first.
    assert!(
        schema_latency < Duration::from_millis(150),
        "get_schema took {schema_latency:?}, which suggests kernel requests \
         are being starved by idle tasks"
    );
}
