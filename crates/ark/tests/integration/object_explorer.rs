//
// object_explorer.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::object_explorer_comm::ChildrenResult;
use amalthea::comm::object_explorer_comm::GetChildrenParams;
use amalthea::comm::object_explorer_comm::ObjectExplorerBackendReply;
use amalthea::comm::object_explorer_comm::ObjectExplorerBackendRequest;
use amalthea::comm::object_explorer_comm::ViewTableParams;
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

/// `View()` on a list opens an object explorer that serves its children,
/// follows reassignments of the variable, and closes when it is removed.
#[test]
fn test_object_explorer_follows_its_variable() {
    let frontend = DummyArkFrontend::lock();
    execute(&frontend, "oe_x <- list(a = 1, b = list(c = 2))");

    frontend.send_execute_request("View(oe_x)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let comm_open = frontend.recv_iopub_comm_open();
    assert_eq!(comm_open.target_name, "positron.objectExplorer");
    assert_eq!(comm_open.data["title"], "oe_x");
    assert_eq!(comm_open.data["variable_path"], serde_json::json!(["oe_x"]));
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    let comm_id = comm_open.comm_id;

    let children = get_children(&frontend, &comm_id);
    let summary: Vec<(String, Option<String>)> = children
        .children
        .into_iter()
        .map(|node| (node.display_name, node.accessor))
        .collect();
    assert_eq!(summary, vec![
        (String::from("a"), Some(String::from(r#"oe_x[["a"]]"#))),
        (String::from("b"), Some(String::from(r#"oe_x[["b"]]"#))),
    ]);

    // Reassigning the variable updates the explorer.
    frontend.send_execute_request("oe_x <- list(a = 2)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let update = frontend.recv_iopub_comm_msg();
    assert_eq!(update.comm_id, comm_id);
    assert_eq!(update.data["method"], "update");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    assert_eq!(get_children(&frontend, &comm_id).total, 1);

    // Removing the variable closes the explorer.
    frontend.send_execute_request("rm(oe_x)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_comm_close(&comm_id);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// An explorer on an environment is updated when the environment changes in
/// place.
#[test]
fn test_object_explorer_follows_environment_mutation() {
    let frontend = DummyArkFrontend::lock();
    execute(&frontend, "oe_env <- new.env(); oe_env$a <- 1");

    frontend.send_execute_request("View(oe_env)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let comm_id = frontend.recv_iopub_comm_open().comm_id;
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // An unrelated execution doesn't update the explorer.
    execute(&frontend, "oe_y <- 1");

    frontend.send_execute_request("oe_env$b <- 2", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let update = frontend.recv_iopub_comm_msg();
    assert_eq!(update.comm_id, comm_id);
    assert_eq!(update.data["method"], "update");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    assert_eq!(get_children(&frontend, &comm_id).total, 2);
}

/// An explorer on an active binding never calls the binding's function.
#[test]
fn test_object_explorer_does_not_run_active_binding() {
    let frontend = DummyArkFrontend::lock();
    execute(
        &frontend,
        "oe_n <- 0; makeActiveBinding('oe_ab', function() { oe_n <<- oe_n + 1; list(a = 1) }, globalenv())",
    );

    frontend.send_execute_request("View(oe_ab)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_comm_open();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    execute(&frontend, "oe_n_after <- oe_n");
    execute(&frontend, "stopifnot(oe_n_after == oe_n, oe_n == 1)");
}

/// A data frame inside a list opens in a data explorer.
#[test]
fn test_object_explorer_view_table() {
    let frontend = DummyArkFrontend::lock();
    execute(&frontend, "oe_tbl <- list(df = data.frame(x = 1:3))");

    frontend.send_execute_request("View(oe_tbl)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let comm_id = frontend.recv_iopub_comm_open().comm_id;
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    let request = ObjectExplorerBackendRequest::ViewTable(ViewTableParams {
        path: vec![String::from("0")],
        title: String::from("df"),
    });
    let mut data = serde_json::to_value(&request).unwrap();
    data["id"] = serde_json::Value::String(String::from("test-rpc"));
    frontend.send_shell_comm_msg(comm_id.clone(), data);
    frontend.recv_iopub_busy();

    let table_open = frontend.recv_iopub_comm_open();
    assert_eq!(table_open.target_name, "positron.dataExplorer");
    assert_eq!(table_open.data["title"], "df");

    let reply = frontend.recv_iopub_comm_msg();
    assert_eq!(reply.comm_id, comm_id);
    assert_eq!(
        serde_json::from_value::<ObjectExplorerBackendReply>(reply.data).unwrap(),
        ObjectExplorerBackendReply::ViewTableReply(table_open.comm_id)
    );
    frontend.recv_iopub_idle();
}

/// Atomic vectors are neither tables nor explorable, so `View()` still errors.
#[test]
fn test_object_explorer_view_atomic_vector_errors() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_execute_request("View(1:3)", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    assert!(frontend
        .recv_iopub_execute_error()
        .contains("Can't `View()`"));
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply_exception();
}

fn execute(frontend: &DummyArkFrontend, code: &str) {
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

fn get_children(frontend: &DummyArkFrontend, comm_id: &str) -> ChildrenResult {
    let request = ObjectExplorerBackendRequest::GetChildren(GetChildrenParams {
        path: vec![],
        start: 0,
        limit: 100,
    });
    let mut data = serde_json::to_value(&request).unwrap();
    data["id"] = serde_json::Value::String(String::from("test-rpc"));

    frontend.send_shell_comm_msg(String::from(comm_id), data);
    frontend.recv_iopub_busy();
    let reply = frontend.recv_iopub_comm_msg();
    frontend.recv_iopub_idle();

    match serde_json::from_value(reply.data).unwrap() {
        ObjectExplorerBackendReply::GetChildrenReply(result) => result,
        other => panic!("Expected GetChildrenReply, got: {other:?}"),
    }
}
