//
// data_explorer.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::data_explorer_comm::DataExplorerBackendReply;
use amalthea::comm::data_explorer_comm::DataExplorerBackendRequest;
use amalthea::comm::data_explorer_comm::DataExplorerFrontendEvent;
use amalthea::comm::data_explorer_comm::GetDataValuesParams;
use amalthea::comm::data_explorer_comm::GetSchemaParams;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket;
use ark::data_explorer::r_data_explorer::RDataExplorer;
use ark::lsp::events::EVENTS;
use ark::r_task;
use crossbeam::channel::bounded;
use harp::assert_match;
use harp::environment::R_ENVS;
use harp::eval::r_parse_eval0;
use harp::object::RObject;
use harp::r_symbol;
use harp::test::start_r;
use libr::R_GlobalEnv;
use libr::Rf_eval;

/// Test helper method to open a built-in dataset in the data explorer.
///
/// Parameters:
/// - dataset: The name of the dataset to open. Must be one of the built-in
///   dataset names returned by `data()`.
///
/// Returns a comm socket that can be used to communicate with the data explorer.
fn open_data_explorer(dataset: String) -> socket::comm::CommSocket {
    // Create a dummy comm manager channel.
    let (comm_manager_tx, comm_manager_rx) = bounded::<CommManagerEvent>(0);

    // Force the dataset to be loaded into the R environment.
    r_task(|| unsafe {
        let data = { RObject::new(Rf_eval(r_symbol!(&dataset), R_GlobalEnv)) };
        RDataExplorer::start(dataset, data, comm_manager_tx).unwrap();
    });

    // Wait for the new comm to show up.
    let msg = comm_manager_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    match msg {
        CommManagerEvent::Opened(socket, _value) => {
            assert_eq!(socket.comm_name, "positron.dataExplorer");
            socket
        },
        _ => panic!("Unexpected Comm Manager Event"),
    }
}

/// Helper method for sending a request to the data explorer and receiving a reply.
///
/// Parameters:
/// - socket: The comm socket to use for communication.
/// - req: The request to send.
fn socket_rpc(
    socket: &socket::comm::CommSocket,
    req: DataExplorerBackendRequest,
) -> DataExplorerBackendReply {
    // Randomly generate a unique ID for this request.
    let id = uuid::Uuid::new_v4().to_string();

    // Serialize the message for the wire
    let json = serde_json::to_value(req).unwrap();
    println!("--> {:?}", json);

    // Covnert the request to a CommMsg and send it.
    let msg = CommMsg::Rpc(String::from(id), json);
    socket.incoming_tx.send(msg).unwrap();
    let msg = socket
        .outgoing_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();

    // Extract the reply from the CommMsg.
    match msg {
        CommMsg::Rpc(_id, value) => {
            println!("<-- {:?}", value);
            let reply: DataExplorerBackendReply = serde_json::from_value(value).unwrap();
            reply
        },
        _ => panic!("Unexpected Comm Message"),
    }
}

/// Runs the data explorer tests.
///
/// Note that these are all run in one single test instead of being split out
/// into multiple tests since they must be run serially.
#[test]
fn test_data_explorer() {
    // Start the R interpreter.
    start_r();

    // --- mtcars ---

    // Open the mtcars data set in the data explorer.
    let socket = open_data_explorer(String::from("mtcars"));

    // Get the schema for the test data set.
    let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
        num_columns: 11,
        start_index: 0,
    });

    // Check that we got the right number of columns.
    assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::GetSchemaReply(schema) => {
            // mtcars is a data frame with 11 columns, so we should get
            // 11 columns back.
            assert_eq!(schema.columns.len(), 11);
        }
    );

    // Get 5 rows of data from the middle of the test data set.
    let req = DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
        row_start_index: 5,
        num_rows: 5,
        column_indices: vec![0, 1, 2, 3, 4],
    });

    // Check that we got the right columns and row labels.
    assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::GetDataValuesReply(data) => {
            assert_eq!(data.columns.len(), 5);
            let labels = data.row_labels.unwrap();
            assert_eq!(labels[0][0], "Valiant");
            assert_eq!(labels[0][1], "Duster 360");
            assert_eq!(labels[0][2], "Merc 240D");
        }
    );

    // --- women ---

    // Open the mtcars data set in the data explorer.
    let socket = open_data_explorer(String::from("women"));

    // Get 2 rows of data from the beginning of the test data set.
    let req = DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
        row_start_index: 0,
        num_rows: 2,
        column_indices: vec![0, 1],
    });

    // Spot check the data values.
    assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::GetDataValuesReply(data) => {
            assert_eq!(data.columns.len(), 2);
            assert_eq!(data.columns[0][1], "59");
            assert_eq!(data.columns[0][2], "60");

            // This data set has no row labels.
            assert!(data.row_labels.is_none());
        }
    );

    // --- updates ---
    let tiny = r_parse_eval0("x <- data.frame(y = 2, z = 3)", R_ENVS.global).unwrap();

    let (comm_manager_tx, comm_manager_rx) = bounded::<CommManagerEvent>(0);
    RDataExplorer::start(String::from("tiny"), tiny, comm_manager_tx).unwrap();

    // Wait for the new comm to show up.
    let msg = comm_manager_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let socket = match msg {
        CommManagerEvent::Opened(socket, _value) => {
            assert_eq!(socket.comm_name, "positron.dataExplorer");
            socket
        },
        _ => panic!("Unexpected Comm Manager Event"),
    };

    // Make a change to the data set.
    r_parse_eval0("x[1, 1] <- 0", R_ENVS.global).unwrap();

    // Emit a console prompt event; this should tickle the data explorer to
    // check for changes.
    EVENTS.console_prompt.emit(());

    // Wait for an update event to arrive
    let msg = socket
        .outgoing_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();
    let msg = match msg {
        CommMsg::Data(value) => {
            let event: DataExplorerFrontendEvent = serde_json::from_value(value).unwrap();
            event
        },
        _ => panic!("Unexpected Comm Message"),
    };

    // Make sure it's a data update event.
    assert_eq!(msg, DataExplorerFrontendEvent::DataUpdate);
}
