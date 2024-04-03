//
// data_explorer.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::data_explorer_comm::ColumnProfileRequest;
use amalthea::comm::data_explorer_comm::ColumnProfileRequestType;
use amalthea::comm::data_explorer_comm::ColumnSortKey;
use amalthea::comm::data_explorer_comm::DataExplorerBackendReply;
use amalthea::comm::data_explorer_comm::DataExplorerBackendRequest;
use amalthea::comm::data_explorer_comm::DataExplorerFrontendEvent;
use amalthea::comm::data_explorer_comm::GetColumnProfilesParams;
use amalthea::comm::data_explorer_comm::GetDataValuesParams;
use amalthea::comm::data_explorer_comm::GetSchemaParams;
use amalthea::comm::data_explorer_comm::SetSortColumnsParams;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket;
use ark::data_explorer::r_data_explorer::DataObjectEnvInfo;
use ark::data_explorer::r_data_explorer::RDataExplorer;
use ark::lsp::events::EVENTS;
use ark::r_task;
use ark::test::r_test;
use ark::thread::RThreadSafe;
use crossbeam::channel::bounded;
use harp::assert_match;
use harp::environment::R_ENVS;
use harp::eval::r_parse_eval0;
use harp::object::RObject;
use harp::r_symbol;
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
        RDataExplorer::start(dataset, data, None, comm_manager_tx).unwrap();
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
    r_test(|| {
        // --- mtcars ---

        let test_mtcars_sort = |socket| {
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

            // Create a request to sort the data set by the 'mpg' column.
            let mpg_sort_keys = vec![ColumnSortKey {
                column_index: 0,
                ascending: true,
            }];
            let req = DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams {
                sort_keys: mpg_sort_keys.clone(),
            });

            // We should get a SetSortColumnsReply back.
            assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetSortColumnsReply() => {});

            // Get the table state and ensure that the backend returns the sort keys
            let req = DataExplorerBackendRequest::GetState;
            assert_match!(socket_rpc(&socket, req),
                DataExplorerBackendReply::GetStateReply(state) => {
                    assert_eq!(state.sort_keys, mpg_sort_keys);
                }
            );

            // Get the first three rows of data from the sorted data set.
            let req = DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
                row_start_index: 0,
                num_rows: 3,
                column_indices: vec![0, 1],
            });

            // Check that sorted values were correctly returned.
            assert_match!(socket_rpc(&socket, req),
                DataExplorerBackendReply::GetDataValuesReply(data) => {
                    // The first three sorted rows should be 10.4, 10.4, and 13.3.
                    assert_eq!(data.columns.len(), 2);
                    assert_eq!(data.columns[0][0], "10.4");
                    assert_eq!(data.columns[0][1], "10.4");
                    assert_eq!(data.columns[0][2], "13.3");

                    // Row labels should be sorted as well.
                    let labels = data.row_labels.unwrap();
                    assert_eq!(labels[0][0], "Cadillac Fleetwood");
                    assert_eq!(labels[0][1], "Lincoln Continental");
                    assert_eq!(labels[0][2], "Camaro Z28");
                }
            );

            // A more complicated sort: sort by 'cyl' in descending order, then by 'mpg'
            // also in descending order.
            let descending_sort_keys = vec![
                ColumnSortKey {
                    column_index: 1,
                    ascending: false,
                },
                ColumnSortKey {
                    column_index: 0,
                    ascending: false,
                },
            ];

            let req = DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams {
                sort_keys: descending_sort_keys.clone(),
            });

            // We should get a SetSortColumnsReply back.
            assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetSortColumnsReply() => {});

            // Get the first three rows of data from the sorted data set.
            let req = DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
                row_start_index: 0,
                num_rows: 3,
                column_indices: vec![0, 1],
            });

            // Check that sorted values were correctly returned.
            assert_match!(socket_rpc(&socket, req),
                DataExplorerBackendReply::GetDataValuesReply(data) => {
                    assert_eq!(data.columns.len(), 2);
                    assert_eq!(data.columns[0][0], "19.2");
                    assert_eq!(data.columns[0][1], "18.7");
                    assert_eq!(data.columns[0][2], "17.3");
                }
            );
        };

        // Test with the regular mtcars data set.
        test_mtcars_sort(open_data_explorer(String::from("mtcars")));

        // --- women ---

        // Open the women data set in the data explorer.
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
                assert_eq!(data.columns[0][0], "58");
                assert_eq!(data.columns[0][1], "59");

                // Row labels should be present.
                let labels = data.row_labels.unwrap();
                assert_eq!(labels[0][0], "1");
                assert_eq!(labels[0][1], "2");
            }
        );

        // --- live updates ---

        // Create a tiny data frame to test live updates.
        let tiny = r_parse_eval0(
            "x <- data.frame(y = c(3, 2, 1), z = c(4, 5, 6))",
            R_ENVS.global,
        )
        .unwrap();

        // Open a data explorer for the tiny data frame and supply a binding to the
        // global environment.
        let (comm_manager_tx, comm_manager_rx) = bounded::<CommManagerEvent>(0);
        let binding = DataObjectEnvInfo {
            name: String::from("x"),
            env: RThreadSafe::new(RObject::view(R_ENVS.global)),
        };
        RDataExplorer::start(String::from("tiny"), tiny, Some(binding), comm_manager_tx).unwrap();

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

        // Make a data-level change to the data set.
        r_parse_eval0("x[1, 1] <- 0", R_ENVS.global).unwrap();

        // Emit a console prompt event; this should tickle the data explorer to
        // check for changes.
        EVENTS.console_prompt.emit(());

        // Wait for an update event to arrive
        assert_match!(socket.outgoing_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            CommMsg::Data(value) => {
                // Make sure it's a data update event.
                assert_match!(serde_json::from_value::<DataExplorerFrontendEvent>(value).unwrap(),
                    DataExplorerFrontendEvent::DataUpdate
                );
        });

        // Create a request to sort the data set by the 'y' column.
        let sort_keys = vec![ColumnSortKey {
            column_index: 0,
            ascending: true,
        }];
        let req = DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams {
            sort_keys: sort_keys.clone(),
        });

        // We should get a SetSortColumnsReply back.
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetSortColumnsReply() => {});

        // Get the values from the first column.
        let req = DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
            row_start_index: 0,
            num_rows: 3,
            column_indices: vec![0],
        });
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetDataValuesReply(data) => {
                assert_eq!(data.columns.len(), 1);
                assert_eq!(data.columns[0][0], "0");
                assert_eq!(data.columns[0][1], "1");
                assert_eq!(data.columns[0][2], "2");
            }
        );

        // Make another data-level change to the data set.
        r_parse_eval0("x[1, 1] <- 3", R_ENVS.global).unwrap();

        // Emit a console prompt event; this should tickle the data explorer to
        // check for changes.
        EVENTS.console_prompt.emit(());

        // Wait for an update event to arrive
        assert_match!(socket.outgoing_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            CommMsg::Data(value) => {
                // Make sure it's a data update event.
                assert_match!(serde_json::from_value::<DataExplorerFrontendEvent>(value).unwrap(),
                    DataExplorerFrontendEvent::DataUpdate
                );
        });

        // Get the values from the first column again. Because a sort is applied,
        // the new value we wrote should be at the end.
        let req = DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
            row_start_index: 0,
            num_rows: 3,
            column_indices: vec![0],
        });
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetDataValuesReply(data) => {
                assert_eq!(data.columns.len(), 1);
                assert_eq!(data.columns[0][0], "1");
                assert_eq!(data.columns[0][1], "2");
                assert_eq!(data.columns[0][2], "3");
            }
        );

        // Now, replace 'x' with an entirely different data set. This should trigger
        // a schema-level update.
        r_parse_eval0(
            "x <- data.frame(y = 'y', z = 'z', three = '3')",
            R_ENVS.global,
        )
        .unwrap();

        // Emit a console prompt event to trigger change detection
        EVENTS.console_prompt.emit(());

        // This should trigger a schema update event.
        assert_match!(socket.outgoing_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            CommMsg::Data(value) => {
                // Make sure it's schema update event.
                assert_match!(serde_json::from_value::<DataExplorerFrontendEvent>(value).unwrap(),
                    DataExplorerFrontendEvent::SchemaUpdate(params) => {
                        assert_eq!(params.discard_state, true);
                    }
                );
        });

        // Get the schema again to make sure it updated. We added a new column, so
        // we should get 3 columns back.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            num_columns: 3,
            start_index: 0,
        });

        // Check that we got the right number of columns.
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetSchemaReply(schema) => {
                assert_eq!(schema.columns.len(), 3);
            }
        );

        // Now, delete 'x' entirely. This should cause the comm to close.
        r_parse_eval0("rm(x)", R_ENVS.global).unwrap();

        // Emit a console prompt event to trigger change detection
        EVENTS.console_prompt.emit(());

        // Wait for an close event to arrive
        assert_match!(socket.outgoing_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            CommMsg::Close => {}
        );

        // --- volcano (a matrix) ---

        // Open the volcano data set in the data explorer. This data set is a matrix.
        let socket = open_data_explorer(String::from("volcano"));

        // Get the schema for the test data set.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            num_columns: 61,
            start_index: 0,
        });

        // Check that we got the right number of columns.
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetSchemaReply(schema) => {
                assert_eq!(schema.columns.len(), 61);
            }
        );

        // Create a request to sort the matrix by the first column.
        let volcano_sort_keys = vec![ColumnSortKey {
            column_index: 0,
            ascending: true,
        }];

        let req = DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams {
            sort_keys: volcano_sort_keys.clone(),
        });

        // We should get a SetSortColumnsReply back.
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetSortColumnsReply() => {});

        // Get the first three rows of data from the sorted matrix.
        let req = DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
            row_start_index: 0,
            num_rows: 4,
            column_indices: vec![0, 1],
        });

        // Check the data values.
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetDataValuesReply(data) => {
                assert_eq!(data.columns.len(), 2);
                assert_eq!(data.columns[0][0], "97");
                assert_eq!(data.columns[0][1], "97");
                assert_eq!(data.columns[0][2], "98");
                assert_eq!(data.columns[0][3], "98");
            }
        );

        // --- null count ---

        // Create a data frame with the Fibonacci sequence, including some NA values
        // where a number in the sequence has been omitted.
        r_parse_eval0(
            "fibo <- data.frame(col = c(1, NA, 2, 3, 5, NA, 13, 21, NA))",
            R_ENVS.global,
        )
        .unwrap();

        // Open the fibo data set in the data explorer.
        let socket = open_data_explorer(String::from("fibo"));

        // Ask for a count of nulls in the first column.
        let req = DataExplorerBackendRequest::GetColumnProfiles(GetColumnProfilesParams {
            profiles: vec![ColumnProfileRequest {
                column_index: 0,
                column_profile_request_type: ColumnProfileRequestType::NullCount,
            }],
        });

        assert_match!(socket_rpc(&socket, req),
           DataExplorerBackendReply::GetColumnProfilesReply(data) => {
               // We asked for the null count of the first column, which has 3 NA values.
               assert!(data.len() == 1);
               assert_eq!(data[0].null_count, Some(3));
           }
        );
    });
}
