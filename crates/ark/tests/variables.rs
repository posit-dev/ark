//
// variables.rs
//
// Copyright (C) 2023-2025 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::comm::variables_comm::ClearParams;
use amalthea::comm::variables_comm::DeleteParams;
use amalthea::comm::variables_comm::QueryTableSummaryParams;
use amalthea::comm::variables_comm::VariablesBackendReply;
use amalthea::comm::variables_comm::VariablesBackendRequest;
use amalthea::comm::variables_comm::VariablesFrontendEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use ark::fixtures::r_test_lock;
use ark::lsp::events::EVENTS;
use ark::r_task::r_task;
use ark::thread::RThreadSafe;
use ark::variables::r_variables::LastValue;
use ark::variables::r_variables::RVariables;
use crossbeam::channel::bounded;
use harp::environment::R_ENVS;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_symbol;
use harp::utils::r_envir_remove;
use harp::utils::r_envir_set;
use libr::R_EmptyEnv;
use libr::R_lsInternal;
use libr::Rboolean_TRUE;
use libr::Rf_ScalarInteger;
use libr::Rf_defineVar;
use libr::Rf_xlength;

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
fn test_variables_list() {
    let _lock = r_test_lock();
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
    let request = VariablesBackendRequest::List;
    let data = serde_json::to_value(request).unwrap();
    let request_id = String::from("refresh-id-1234");
    incoming_tx
        .send(CommMsg::Rpc(request_id.clone(), data))
        .unwrap();

    // The test might receive an update event before the RPC response; consume
    // any update events first
    let mut msg = outgoing_rx.recv().unwrap();
    while let CommMsg::Data(_) = msg {
        // Continue receiving until we get the RPC response
        msg = outgoing_rx.recv().unwrap();
    }

    let data = match msg {
        CommMsg::Rpc(reply_id, data) => {
            // Ensure that the reply ID we received from then environment pane
            // matches the request ID we sent
            assert_eq!(request_id, reply_id);
            data
        },
        _ => panic!("Expected RPC message, got {:?}", msg),
    };

    // Unmarshal the list and check for the variable we created
    let reply: VariablesBackendReply = serde_json::from_value(data).unwrap();
    match reply {
        VariablesBackendReply::ListReply(list) => {
            // Now version can vary based on the threading
            println!("List version: {:?}", list.version);

            // Check that the "everything" variable is in the list
            let var = list
                .variables
                .iter()
                .find(|v| v.display_name == "everything");
            assert!(var.is_some(), "Couldn't find 'everything' variable");

            // No need to check the exact version number as it might vary
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
        },
        _ => panic!("Expected update event"),
    }

    // Request that the environment be cleared
    let clear = VariablesBackendRequest::Clear(ClearParams {
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
    let reply: VariablesBackendReply = serde_json::from_value(data).unwrap();
    match reply {
        VariablesBackendReply::ClearReply() => {},
        _ => panic!("Expected clear reply"),
    }

    // test the env is now empty
    r_task(|| unsafe {
        let test_env = test_env.get().clone();
        let contents = RObject::new(R_lsInternal(*test_env, Rboolean_TRUE));
        assert_eq!(Rf_xlength(*contents), 0);
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
        },
        _ => panic!("Expected update event"),
    }

    // Request that a environment be deleted
    let delete = VariablesBackendRequest::Delete(DeleteParams {
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

    let reply: VariablesBackendReply = serde_json::from_value(data).unwrap();

    match reply {
        VariablesBackendReply::DeleteReply(update) => {
            assert_eq!(update, ["a"]);
        },
        _ => panic!("Expected delete reply"),
    };

    // Close the comm. Otherwise the thread panics
    incoming_tx.send(CommMsg::Close).unwrap();
}

/**
 * Test for the .Last.value feature with the option enabled.
 *
 * This test:
 * 1. Creates a new REnvironment with show_last_value=true
 * 2. Ensures that the environment list includes .Last.value
 * 3. Verifies that .Last.value appears even when creating other variables
 *
 */
#[test]
fn test_variables_last_value_enabled() {
    let _lock = r_test_lock();

    // Create a new environment for the test
    let test_env = r_task(|| unsafe {
        let env = RFunction::new("base", "new.env")
            .param("parent", R_EmptyEnv)
            .call()
            .unwrap();
        RThreadSafe::new(env)
    });

    // Create a sender/receiver pair for the comm channel
    let comm = CommSocket::new(
        CommInitiator::FrontEnd,
        String::from("test-last-value-enabled-comm-id"),
        String::from("positron.environment"),
    );

    // Create a dummy comm manager channel
    let (comm_manager_tx, _) = bounded::<CommManagerEvent>(0);

    // Create a new environment handler with show_last_value=true
    let incoming_tx = comm.incoming_tx.clone();
    let outgoing_rx = comm.outgoing_rx.clone();
    r_task(|| {
        let test_env = test_env.get().clone();
        RVariables::start_with_config(
            test_env,
            comm.clone(),
            comm_manager_tx.clone(),
            LastValue::Always,
        );
    });

    // Ensure we get a list of variables after initialization
    let msg = outgoing_rx.recv().unwrap();
    let data = match msg {
        CommMsg::Data(data) => data,
        _ => panic!("Expected data message"),
    };

    // Verify that .Last.value is included in the initial variable list
    let evt: VariablesFrontendEvent = serde_json::from_value(data).unwrap();
    match evt {
        VariablesFrontendEvent::Refresh(params) => {
            assert_eq!(params.variables.len(), 1);
            assert_eq!(params.variables[0].display_name, ".Last.value");
            assert_eq!(params.version, 1);
        },
        _ => panic!("Expected refresh event"),
    }

    // Create a variable in the R environment
    r_task(|| unsafe {
        let test_env = test_env.get().clone();
        let sym = r_symbol!("test_var");
        Rf_defineVar(sym, Rf_ScalarInteger(99), *test_env);
    });

    // Simulate a prompt signal
    EVENTS.console_prompt.emit(());

    // Wait for the update event
    let msg = outgoing_rx.recv().unwrap();
    let data = match msg {
        CommMsg::Data(data) => data,
        _ => panic!("Expected data message"),
    };

    // We might get multiple update events - first for .Last.value, then for test_var
    // Rather than assert on specific quantities, just verify that eventually
    // both .Last.value and test_var appear

    // Create sets to accumulate variables we've seen
    let mut seen_last_value = false;
    let mut seen_test_var = false;

    // Process first update
    let evt: VariablesFrontendEvent = serde_json::from_value(data).unwrap();
    match evt {
        VariablesFrontendEvent::Update(params) => {
            println!("Update params: {:?}", params);

            // Update what we've seen
            seen_last_value = params
                .assigned
                .iter()
                .any(|v| v.display_name == ".Last.value") ||
                seen_last_value;

            seen_test_var =
                params.assigned.iter().any(|v| v.display_name == "test_var") || seen_test_var;
        },
        _ => panic!("Expected update event"),
    }

    // If we haven't seen both variables yet, try to get a second update
    if !seen_last_value || !seen_test_var {
        // It's possible that we won't get another update, so use a timeout
        if let Ok(msg) = outgoing_rx.recv_timeout(std::time::Duration::from_millis(500)) {
            if let CommMsg::Data(data) = msg {
                let evt: VariablesFrontendEvent = serde_json::from_value(data).unwrap();
                if let VariablesFrontendEvent::Update(params) = evt {
                    println!("Second update params: {:?}", params);

                    // Update what we've seen
                    seen_last_value = params
                        .assigned
                        .iter()
                        .any(|v| v.display_name == ".Last.value") ||
                        seen_last_value;

                    seen_test_var = params.assigned.iter().any(|v| v.display_name == "test_var") ||
                        seen_test_var;
                }
            }
        }
    }

    // Assert that we've seen both variables
    assert!(seen_last_value, "Never saw .Last.value in any update");
    assert!(seen_test_var, "Never saw test_var in any update");

    // Close the comm
    incoming_tx.send(CommMsg::Close).unwrap();
}

/**
 * Test for the .Last.value feature with the option disabled.
 *
 * This test:
 * 1. Creates a new REnvironment with show_last_value=false (default)
 * 2. Ensures that the environment list does not include .Last.value
 *
 */
#[test]
fn test_variables_last_value_disabled() {
    // Create a new environment for the test
    let test_env = r_task(|| unsafe {
        let env = RFunction::new("base", "new.env")
            .param("parent", R_EmptyEnv)
            .call()
            .unwrap();
        RThreadSafe::new(env)
    });

    // Create a sender/receiver pair for the comm channel
    let comm = CommSocket::new(
        CommInitiator::FrontEnd,
        String::from("test-last-value-disabled-comm-id"),
        String::from("positron.environment"),
    );

    // Create a dummy comm manager channel
    let (comm_manager_tx, _) = bounded::<CommManagerEvent>(0);

    // Create a new environment handler (default show_last_value=false)
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

    // Verify that .Last.value is NOT included in the initial variable list
    let evt: VariablesFrontendEvent = serde_json::from_value(data).unwrap();
    match evt {
        VariablesFrontendEvent::Refresh(params) => {
            assert_eq!(params.variables.len(), 0);
            assert_eq!(params.version, 1);
        },
        _ => panic!("Expected refresh event"),
    }

    // Create a variable in the R environment
    r_task(|| unsafe {
        let test_env = test_env.get().clone();
        let sym = r_symbol!("test_var");
        Rf_defineVar(sym, Rf_ScalarInteger(99), *test_env);
    });

    // Simulate a prompt signal
    EVENTS.console_prompt.emit(());

    // Wait for the update event
    let msg = outgoing_rx.recv().unwrap();
    let data = match msg {
        CommMsg::Data(data) => data,
        _ => panic!("Expected data message"),
    };

    // Verify that .Last.value is NOT included in the updated variable list
    let evt: VariablesFrontendEvent = serde_json::from_value(data).unwrap();
    match evt {
        VariablesFrontendEvent::Update(params) => {
            assert_eq!(params.assigned.len(), 1);

            // Check that .Last.value is NOT in the assigned list
            let last_value = params
                .assigned
                .iter()
                .find(|v| v.display_name == ".Last.value");
            assert!(last_value.is_none());

            // Check that test_var is in the list
            let test_var = params
                .assigned
                .iter()
                .find(|v| v.display_name == "test_var");
            assert!(test_var.is_some());
        },
        _ => panic!("Expected update event"),
    }

    // Close the comm
    incoming_tx.send(CommMsg::Close).unwrap();
}

#[test]
fn test_query_table_summary() {
    let _lock = r_test_lock();

    // Create a sender/receiver pair for the comm channel
    let comm = CommSocket::new(
        CommInitiator::FrontEnd,
        String::from("test-table-summary-comm-id"),
        String::from("positron.environment"),
    );
    let incoming_tx = comm.incoming_tx.clone();
    let outgoing_rx = comm.outgoing_rx.clone();

    // Simulate comm manager
    let (comm_manager_tx, _) = bounded::<CommManagerEvent>(0);

    r_task(|| {
        // Create a new variables comm
        RVariables::start(RObject::from(R_ENVS.global), comm.clone(), comm_manager_tx);

        // Create test datasets
        let code = r#"
        test_df <- data.frame(
            numeric_col = c(1.5, 2.5, 3.5, NA),
            integer_col = c(1L, 2L, 3L, 4L),
            character_col = c('a', 'b', 'c', ''),
            logical_col = c(TRUE, FALSE, TRUE, NA),
            stringsAsFactors = FALSE
        )
        "#;
        harp::parse_eval_global(code).unwrap();

        let code = r#"
        test_matrix <- matrix(
            1:12,
            nrow = 4,
            ncol = 3,
            dimnames = list(
                c('row1', 'row2', 'row3', 'row4'),
                c('col1', 'col2', 'col3')
            )
        )"#;

        harp::parse_eval_global(code).unwrap();
    });

    // Simulate a prompt signal to refresh the variable list
    // and consume the update event
    EVENTS.console_prompt.emit(());
    let _ = outgoing_rx.recv().unwrap();

    // --- TEST 1: Query summary for data.frame with summary_stats query type ---

    // Request table summary for data.frame
    let query_df = VariablesBackendRequest::QueryTableSummary(QueryTableSummaryParams {
        path: vec![String::from("test_df")],
        query_types: vec![String::from("summary_stats")],
    });

    let data = serde_json::to_value(query_df).unwrap();
    let request_id = String::from("df-summary-id");

    incoming_tx
        .send(CommMsg::Rpc(request_id.clone(), data))
        .unwrap();

    // Get the response
    let data = match outgoing_rx.recv().unwrap() {
        CommMsg::Rpc(reply_id, data) => {
            assert_eq!(request_id, reply_id);
            data
        },
        _ => panic!("Expected RPC message"),
    };

    // Verify the response
    let reply: VariablesBackendReply = serde_json::from_value(data).unwrap();
    match reply {
        VariablesBackendReply::QueryTableSummaryReply(result) => {
            assert_eq!(result.num_rows, 4);
            assert_eq!(result.num_columns, 4);
            assert_eq!(result.column_schemas.len(), 4);
            assert_eq!(result.column_profiles.len(), 4);

            let schemas: Vec<serde_json::Value> = result
                .column_schemas
                .iter()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect();

            assert_eq!(schemas[0]["column_name"], "numeric_col");
            assert_eq!(schemas[1]["column_name"], "integer_col");
            assert_eq!(schemas[2]["column_name"], "character_col");
            assert_eq!(schemas[3]["column_name"], "logical_col");

            assert_eq!(schemas[0]["type_display"], "number");
            assert_eq!(schemas[1]["type_display"], "number");
            assert_eq!(schemas[2]["type_display"], "string");
            assert_eq!(schemas[3]["type_display"], "boolean");

            let profiles: Vec<serde_json::Value> = result
                .column_profiles
                .iter()
                .map(|p| serde_json::from_str(p).unwrap())
                .collect();

            // Check that summary stats exists for each column
            for profile in &profiles {
                assert!(profile["summary_stats"].is_object());
            }

            // Check numeric column stats
            let numeric_stats = &profiles[0]["summary_stats"];
            assert!(numeric_stats["number_stats"].is_object());

            // Check logical column stats
            let logical_stats = &profiles[3]["summary_stats"];
            assert!(logical_stats["boolean_stats"].is_object());
        },

        _ => panic!("Expected QueryTableSummaryReply"),
    }

    // --- TEST 2: Query summary for matrix with empty query type list ---

    // Request table summary for matrix without summary_stats
    let query_matrix = VariablesBackendRequest::QueryTableSummary(QueryTableSummaryParams {
        path: vec![String::from("test_matrix")],
        query_types: vec![], // Empty query types list
    });

    let data = serde_json::to_value(query_matrix).unwrap();
    let request_id = String::from("matrix-summary-id");

    incoming_tx
        .send(CommMsg::Rpc(request_id.clone(), data))
        .unwrap();

    // Get the response
    let data = match outgoing_rx.recv().unwrap() {
        CommMsg::Rpc(reply_id, data) => {
            assert_eq!(request_id, reply_id);
            data
        },
        _ => panic!("Expected RPC message"),
    };

    // Verify the response
    let reply: VariablesBackendReply = serde_json::from_value(data).unwrap();
    match reply {
        VariablesBackendReply::QueryTableSummaryReply(result) => {
            // Check basic structure
            assert_eq!(result.num_rows, 4);
            assert_eq!(result.num_columns, 3);
            assert_eq!(result.column_schemas.len(), 3);

            // No profiles should be generated (empty query_types)
            assert_eq!(result.column_profiles.len(), 0);

            // Parse and check column schemas
            let schemas: Vec<serde_json::Value> = result
                .column_schemas
                .iter()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect();

            // Check column names match the dimnames we set
            assert_eq!(schemas[0]["column_name"], "col1");
            assert_eq!(schemas[1]["column_name"], "col2");
            assert_eq!(schemas[2]["column_name"], "col3");

            // Matrix should have numeric columns
            for schema in &schemas {
                assert_eq!(schema["type_display"], "number");
            }
        },
        _ => panic!("Expected QueryTableSummaryReply"),
    }

    // --- TEST 3: Test query for an object that is not a table ---

    // Create a non-table object
    r_task(|| {
        let code = "non_table_obj <- list(a = 1, b = 2)";
        harp::parse_eval_global(code).unwrap();
    });

    // Simulate a prompt signal to refresh the variable list
    // and consume the update event
    EVENTS.console_prompt.emit(());
    let _ = outgoing_rx.recv().unwrap();

    // Request table summary for non-table object
    let query_non_table = VariablesBackendRequest::QueryTableSummary(QueryTableSummaryParams {
        path: vec![String::from("non_table_obj")],
        query_types: vec![String::from("summary_stats")],
    });

    let data = serde_json::to_value(query_non_table).unwrap();
    let request_id = String::from("non-table-summary-id");

    incoming_tx
        .send(CommMsg::Rpc(request_id.clone(), data))
        .unwrap();

    // Get the error response
    let data = match outgoing_rx.recv().unwrap() {
        CommMsg::Rpc(reply_id, data) => {
            assert_eq!(request_id, reply_id);
            data
        },
        _ => panic!("Expected RPC message"),
    };

    // Check if the response contains an error field (JSON-RPC error)
    if data.get("error").is_some() {
        let error_message = data["error"]["message"].as_str().unwrap();
        assert!(error_message.contains("not a supported table type"));
    } else {
        // If no error, it must be a successful reply which is unexpected
        panic!("Expected error response, but got successful reply");
    }

    // Close the comm
    incoming_tx.send(CommMsg::Close).unwrap();

    // Clean up
    r_task(|| {
        let code = "rm(test_df, test_matrix, non_table_obj)";
        harp::parse_eval_global(code).unwrap();
    });
}
