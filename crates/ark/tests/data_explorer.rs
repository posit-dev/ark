//
// data_explorer.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::data_explorer_comm::DataExplorerBackendReply;
use amalthea::comm::data_explorer_comm::DataExplorerBackendRequest;
use amalthea::comm::data_explorer_comm::GetDataValuesParams;
use amalthea::comm::data_explorer_comm::GetSchemaParams;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket;
use ark::data_explorer::r_data_explorer::RDataExplorer;
use ark::r_task;
use crossbeam::channel::bounded;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::test::start_r;
use harp::utils::r_envir_get;
use libr::R_GlobalEnv;

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

    // Covnert the request to a CommMsg and send it.
    let msg = CommMsg::Rpc(String::from(id), serde_json::to_value(req).unwrap());
    socket.incoming_tx.send(msg).unwrap();
    let msg = socket
        .outgoing_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();

    // Extract the reply from the CommMsg.
    match msg {
        CommMsg::Rpc(_id, value) => {
            let reply: DataExplorerBackendReply = serde_json::from_value(value).unwrap();
            reply
        },
        _ => panic!("Unexpected Comm Message"),
    }
}

#[test]
fn test_data_explorer() {
    // Start the R interpreter
    start_r();

    // Create a dummy comm manager channel.
    let (comm_manager_tx, comm_manager_rx) = bounded::<CommManagerEvent>(0);

    // Force the mtcars dataset to make it available. This is a sample dataset
    // that comes with R.
    r_task(|| unsafe {
        let data = { r_envir_get("mtcars", R_GlobalEnv).unwrap() };
        let mtcars = RFunction::new("base", "force")
            .param("x", data)
            .call()
            .unwrap();
        // Make sure this looks like the mtcars dataset.
        assert_eq!(mtcars.length(), 11);
        RDataExplorer::start(String::from("test"), mtcars, comm_manager_tx).unwrap();
    });

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

    // Get the schema for the test data set.
    let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
        num_columns: 11,
        start_index: 0,
    });
    let reply = socket_rpc(&socket, req);
    match reply {
        DataExplorerBackendReply::GetSchemaReply(schema) => {
            // mtcars is a data frame with 11 columns, so we should get
            // 11 columns back.
            assert_eq!(schema.columns.len(), 11);
        },
        _ => panic!("Unexpected Data Explorer Reply: {:?}", reply),
    }

    // Get 5 rows of data from the middle of the test data set.
    let req = DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
        row_start_index: 5,
        num_rows: 5,
        column_indices: vec![0, 1, 2, 3, 4],
    });
    let reply = socket_rpc(&socket, req);
    match reply {
        DataExplorerBackendReply::GetDataValuesReply(data) => {
            assert_eq!(data.columns.len(), 5);
        },
        _ => panic!("Unexpected Data Explorer Reply: {:?}", reply),
    }
}
