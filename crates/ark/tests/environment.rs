//
// environment.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::comm::variables_comm::ClearParams;
use amalthea::comm::variables_comm::DeleteParams;
use amalthea::comm::variables_comm::VariablesBackendRpcReply;
use amalthea::comm::variables_comm::VariablesBackendRpcRequest;
use amalthea::comm::variables_comm::VariablesFrontendEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use ark::lsp::events::EVENTS;
use ark::r_task;
use ark::thread::RThreadSafe;
use ark::variables::r_variables::RVariables;
use crossbeam::channel::bounded;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_symbol;
use harp::test::start_r;
use harp::utils::r_envir_remove;
use harp::utils::r_envir_set;
use libR_shim::*;

/**
 * Basic test for the R environment list. This test:
 *
 * 1. Starts the R interpreter
 * 2. Creates a new REnvironment
 * 3. Ensures that the environment list is empty
 * 4. Creates a variable in the R environment
 * 5. Ensures that the environment list contains the new variable
 */
#[test]
fn test_environment_list() {
    // Start the R interpreter so we have a live environment for the test to run
    // against.
    start_r();

    // Create a new environment for the test. We use a new, empty environment
    // (with the empty environment as its parent) so that each test in this
    // file can run independently.
    let test_env = r_task(|| unsafe {
        let env = RFunction::new("base", "new.env")
            .param("parent", R_EmptyEnv)
            .call()
            .unwrap();
        RThreadSafe::new(env)
    });

    // Create a sender/receiver pair for the comm channel.
    let comm = CommSocket::new(
        CommInitiator::FrontEnd,
        String::from("test-environment-comm-id"),
        String::from("positron.environment"),
    );

    // Create a dummy comm manager channel that isn't actually used.
    // It's required when opening a `RDataViewer` comm through `view()`, but
    // we don't test that here.
    let (comm_manager_tx, _) = bounded::<CommManagerEvent>(0);

    // Create a new environment handler and give it the test
    // environment we created.
    let incoming_tx = comm.incoming_tx.clone();
    let outgoing_rx = comm.outgoing_rx.clone();
    r_task(|| {
        let test_env = test_env.get().clone();
        RVariables::start(test_env, comm.clone(), comm_manager_tx.clone());
    });

    // Ensure we get a list of variables after initialization
    let msg = outgoing_rx.recv().unwrap();
    let data = match msg {
        CommMsg::Data(data) => data,
        _ => panic!("Expected data message"),
    };

    // Ensure we got a list of variables by unmarshalling the JSON. The list
    // should be empty since we don't have any variables in the R environment.
    let evt: VariablesFrontendEvent = serde_json::from_value(data).unwrap();
    match evt {
        VariablesFrontendEvent::Refresh(params) => {
            assert!(params.variables.len() == 0);
            assert_eq!(params.version, 1);
        },
        _ => panic!("Expected refresh event"),
    }

    // Now create a variable in the R environment and ensure we get a list of
    // variables with the new variable in it.
    r_task(|| unsafe {
        let test_env = test_env.get().clone();
        let sym = r_symbol!("everything");
        Rf_defineVar(sym, Rf_ScalarInteger(42), *test_env);
    });

    // Request a list of variables
    let request = VariablesBackendRpcRequest::List;
    let data = serde_json::to_value(request).unwrap();
    let request_id = String::from("refresh-id-1234");
    incoming_tx
        .send(CommMsg::Rpc(request_id.clone(), data))
        .unwrap();

    // Wait for the new list of variables to be delivered
    let msg = outgoing_rx.recv().unwrap();
    let data = match msg {
        CommMsg::Rpc(reply_id, data) => {
            // Ensure that the reply ID we received from then environment pane
            // matches the request ID we sent
            assert_eq!(request_id, reply_id);
            data
        },
        _ => panic!("Expected data message, got {:?}", msg),
    };

    // Unmarshal the list and check for the variable we created
    let reply: VariablesBackendRpcReply = serde_json::from_value(data).unwrap();
    match reply {
        VariablesBackendRpcReply::ListReply(list) => {
            assert!(list.variables.len() == 1);
            let var = &list.variables[0];
            assert_eq!(var.display_name, "everything");
            assert_eq!(list.version, Some(2));
        },
        _ => panic!("Expected list reply"),
    }

    // Create another variable
    r_task(|| unsafe {
        let test_env = test_env.get().clone();
        r_envir_set("nothing", Rf_ScalarInteger(43), *test_env);
        r_envir_remove("everything", *test_env);
    });

    // Simulate a prompt signal
    EVENTS.console_prompt.emit(());

    // Wait for the new list of variables to be delivered
    let msg = outgoing_rx.recv().unwrap();
    let data = match msg {
        CommMsg::Data(data) => data,
        _ => panic!("Expected data message, got {:?}", msg),
    };

    // Unmarshal the list and check for the variable we created
    let evt: VariablesFrontendEvent = serde_json::from_value(data).unwrap();
    match evt {
        VariablesFrontendEvent::Update(params) => {
            assert_eq!(params.assigned.len(), 1);
            assert_eq!(params.removed.len(), 1);
            assert_eq!(params.assigned[0].display_name, "nothing");
            assert_eq!(params.removed[0], "everything");
            assert_eq!(params.version, 3);
        },
        _ => panic!("Expected update event"),
    }

    // Request that the environment be cleared
    let clear = VariablesBackendRpcRequest::Clear(ClearParams {
        include_hidden_objects: true,
    });
    let data = serde_json::to_value(clear).unwrap();
    let request_id = String::from("clear-id-1235");
    incoming_tx
        .send(CommMsg::Rpc(request_id.clone(), data))
        .unwrap();

    // Wait up to 1s for the comm to send us an update message
    let msg = outgoing_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let data = match msg {
        CommMsg::Data(data) => data,
        _ => panic!("Expected data message, got {:?}", msg),
    };

    // Ensure we get an event notifying us of the change
    let evt: VariablesFrontendEvent = serde_json::from_value(data).unwrap();
    match evt {
        VariablesFrontendEvent::Update(params) => {
            assert_eq!(params.assigned.len(), 0);
            assert_eq!(params.removed.len(), 1);
            assert_eq!(params.version, 4);
        },
        _ => panic!("Expected update event"),
    }

    // Wait for the success message to be delivered
    let data = match outgoing_rx.recv().unwrap() {
        CommMsg::Rpc(reply_id, data) => {
            // Ensure that the reply ID we received from then environment pane
            // matches the request ID we sent
            assert_eq!(request_id, reply_id);

            data
        },
        _ => panic!("Expected RPC message"),
    };

    // Ensure we get a reply
    let reply: VariablesBackendRpcReply = serde_json::from_value(data).unwrap();
    match reply {
        VariablesBackendRpcReply::ClearReply() => {},
        _ => panic!("Expected clear reply"),
    }

    // test the env is now empty
    r_task(|| unsafe {
        let test_env = test_env.get().clone();
        let contents = RObject::new(R_lsInternal(*test_env, Rboolean_TRUE));
        assert_eq!(Rf_length(*contents), 0);
    });

    // Create some more variables
    r_task(|| unsafe {
        let test_env = test_env.get().clone();

        let sym = r_symbol!("a");
        Rf_defineVar(sym, Rf_ScalarInteger(42), *test_env);

        let sym = r_symbol!("b");
        Rf_defineVar(sym, Rf_ScalarInteger(43), *test_env);
    });

    // Simulate a prompt signal
    EVENTS.console_prompt.emit(());

    let msg = outgoing_rx.recv().unwrap();
    let data = match msg {
        CommMsg::Data(data) => data,
        _ => panic!("Expected data message, got {:?}", msg),
    };

    let evt: VariablesFrontendEvent = serde_json::from_value(data).unwrap();
    match evt {
        VariablesFrontendEvent::Update(params) => {
            assert_eq!(params.assigned.len(), 2);
            assert_eq!(params.removed.len(), 0);
            assert_eq!(params.version, 5);
        },
        _ => panic!("Expected update event"),
    }

    // Request that a environment be deleted
    let delete = VariablesBackendRpcRequest::Delete(DeleteParams {
        names: vec![String::from("a")],
    });
    let data = serde_json::to_value(delete).unwrap();
    let request_id = String::from("delete-id-1236");
    incoming_tx
        .send(CommMsg::Rpc(request_id.clone(), data))
        .unwrap();

    let data = match outgoing_rx.recv().unwrap() {
        CommMsg::Rpc(reply_id, data) => {
            assert_eq!(request_id, reply_id);
            data
        },
        _ => panic!("Expected RPC message"),
    };

    let reply: VariablesBackendRpcReply = serde_json::from_value(data).unwrap();

    match reply {
        VariablesBackendRpcReply::DeleteReply(update) => {
            assert_eq!(update, ["a"]);
        },
        _ => panic!("Expected delete reply"),
    };

    // Close the comm. Otherwise the thread panics
    incoming_tx.send(CommMsg::Close).unwrap();
}
