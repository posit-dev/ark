//
// data_explorer.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//
use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::data_explorer_comm::ArraySelection;
use amalthea::comm::data_explorer_comm::ColumnFrequencyTable;
use amalthea::comm::data_explorer_comm::ColumnFrequencyTableParams;
use amalthea::comm::data_explorer_comm::ColumnHistogram;
use amalthea::comm::data_explorer_comm::ColumnHistogramParams;
use amalthea::comm::data_explorer_comm::ColumnHistogramParamsMethod;
use amalthea::comm::data_explorer_comm::ColumnProfileParams;
use amalthea::comm::data_explorer_comm::ColumnProfileRequest;
use amalthea::comm::data_explorer_comm::ColumnProfileSpec;
use amalthea::comm::data_explorer_comm::ColumnProfileType;
use amalthea::comm::data_explorer_comm::ColumnSelection;
use amalthea::comm::data_explorer_comm::ColumnSortKey;
use amalthea::comm::data_explorer_comm::ColumnValue;
use amalthea::comm::data_explorer_comm::DataExplorerBackendReply;
use amalthea::comm::data_explorer_comm::DataExplorerBackendRequest;
use amalthea::comm::data_explorer_comm::DataExplorerFrontendEvent;
use amalthea::comm::data_explorer_comm::DataSelectionIndices;
use amalthea::comm::data_explorer_comm::DataSelectionRange;
use amalthea::comm::data_explorer_comm::DataSelectionSingleCell;
use amalthea::comm::data_explorer_comm::ExportDataSelectionParams;
use amalthea::comm::data_explorer_comm::ExportFormat;
use amalthea::comm::data_explorer_comm::ExportedData;
use amalthea::comm::data_explorer_comm::FilterComparison;
use amalthea::comm::data_explorer_comm::FilterComparisonOp;
use amalthea::comm::data_explorer_comm::FilterResult;
use amalthea::comm::data_explorer_comm::FilterTextSearch;
use amalthea::comm::data_explorer_comm::FormatOptions;
use amalthea::comm::data_explorer_comm::GetColumnProfilesParams;
use amalthea::comm::data_explorer_comm::GetDataValuesParams;
use amalthea::comm::data_explorer_comm::GetRowLabelsParams;
use amalthea::comm::data_explorer_comm::GetSchemaParams;
use amalthea::comm::data_explorer_comm::RowFilter;
use amalthea::comm::data_explorer_comm::RowFilterCondition;
use amalthea::comm::data_explorer_comm::RowFilterParams;
use amalthea::comm::data_explorer_comm::RowFilterType;
use amalthea::comm::data_explorer_comm::Selection;
use amalthea::comm::data_explorer_comm::SetRowFiltersParams;
use amalthea::comm::data_explorer_comm::SetSortColumnsParams;
use amalthea::comm::data_explorer_comm::SummaryStatsBoolean;
use amalthea::comm::data_explorer_comm::SummaryStatsNumber;
use amalthea::comm::data_explorer_comm::SummaryStatsString;
use amalthea::comm::data_explorer_comm::TableSelection;
use amalthea::comm::data_explorer_comm::TableSelectionKind;
use amalthea::comm::data_explorer_comm::TextSearchType;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket;
use ark::data_explorer::format::format_string;
use ark::data_explorer::r_data_explorer::DataObjectEnvInfo;
use ark::data_explorer::r_data_explorer::RDataExplorer;
use ark::lsp::events::EVENTS;
use ark::r_task::r_task;
use ark::test::r_test;
use ark::test::socket_rpc_request;
use ark::thread::RThreadSafe;
use crossbeam::channel::bounded;
use harp::assert_match;
use harp::environment::R_ENVS;
use harp::eval::r_parse_eval0;
use harp::object::RObject;
use harp::r_symbol;
use itertools::enumerate;
use itertools::Itertools;
use libr::R_GlobalEnv;
use libr::Rf_eval;

struct DataExplorerSocket {
    socket: socket::comm::CommSocket,
}

impl Drop for DataExplorerSocket {
    fn drop(&mut self) {
        self.socket.incoming_tx.send(CommMsg::Close).unwrap();
    }
}

/// Test helper method to open a built-in dataset in the data explorer.
///
/// Parameters:
/// - dataset: The name of the dataset to open. Must be one of the built-in
///   dataset names returned by `data()`.
///
/// Returns a comm socket that can be used to communicate with the data explorer.
fn open_data_explorer(dataset: String) -> DataExplorerSocket {
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
            DataExplorerSocket { socket }
        },
        _ => panic!("Unexpected Comm Manager Event"),
    }
}

fn open_data_explorer_from_expression(
    expr: &str,
    bind: Option<&str>,
) -> anyhow::Result<DataExplorerSocket> {
    let object = r_parse_eval0(expr, R_ENVS.global)?;

    let binding = match bind {
        Some(name) => Some(DataObjectEnvInfo {
            name: name.to_string(),
            env: RThreadSafe::new(RObject::view(R_ENVS.global)),
        }),
        None => None,
    };

    let (comm_manager_tx, comm_manager_rx) = bounded::<CommManagerEvent>(0);
    RDataExplorer::start(String::from("obj"), object, binding, comm_manager_tx).unwrap();

    // Wait for the new comm to show up.
    let msg = comm_manager_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();

    match msg {
        CommManagerEvent::Opened(socket, _value) => {
            assert_eq!(socket.comm_name, "positron.dataExplorer");
            Ok(DataExplorerSocket { socket })
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
    socket: &DataExplorerSocket,
    req: DataExplorerBackendRequest,
) -> DataExplorerBackendReply {
    socket_rpc_request::<DataExplorerBackendRequest, DataExplorerBackendReply>(&socket.socket, req)
}

fn default_format_options() -> FormatOptions {
    FormatOptions {
        large_num_digits: 2,
        small_num_digits: 4,
        max_integral_digits: 7,
        thousands_sep: Some(",".to_string()),
        max_value_length: 100,
    }
}

fn get_data_values_request(
    row_start_index: i64,
    num_rows: i64,
    column_indices: Vec<i64>,
    format_options: FormatOptions,
) -> DataExplorerBackendRequest {
    let columns = column_indices
        .into_iter()
        .map(|column_index| ColumnSelection {
            column_index,
            spec: ArraySelection::SelectRange(DataSelectionRange {
                first_index: row_start_index,
                last_index: row_start_index + num_rows - 1,
            }),
        })
        .collect();

    DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
        columns,
        format_options,
    })
}

fn test_mtcars_sort(socket: DataExplorerSocket, has_row_names: bool, display_name: String) {
    // Get the schema for the test data set.
    let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
        column_indices: vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
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
    let req = get_data_values_request(5, 5, vec![0, 1, 2, 3, 4], default_format_options());

    // Check that we got the right columns and row labels.
    assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::GetDataValuesReply(data) => {
            assert_eq!(data.columns.len(), 5);
        }
    );

    // Check row names are present
    if has_row_names {
        let req = DataExplorerBackendRequest::GetRowLabels(GetRowLabelsParams {
            selection: ArraySelection::SelectIndices(DataSelectionIndices {
                indices: vec![5, 6, 7, 8, 9],
            }),
            format_options: default_format_options(),
        });
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetRowLabelsReply(row_labels) => {
                let labels = row_labels.row_labels;
                assert_eq!(labels[0][0], "Valiant");
                assert_eq!(labels[0][1], "Duster 360");
                assert_eq!(labels[0][2], "Merc 240D");
            }
        );
    }

    // Create a request to sort the data set by the 'mpg' column.
    let mpg_sort_keys = vec![ColumnSortKey {
        column_index: 0,
        ascending: true,
    }];
    let req = DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams {
        sort_keys: mpg_sort_keys.clone(),
    });

    // We should get a SetSortColumnsReply back.
    assert_match!(socket_rpc(&socket, req), DataExplorerBackendReply::SetSortColumnsReply() => {});

    // Get the table state and ensure that the backend returns the sort keys
    let req = DataExplorerBackendRequest::GetState;
    assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::GetStateReply(state) => {
            assert_eq!(state.display_name, display_name);
            assert_eq!(state.sort_keys, mpg_sort_keys);
        }
    );

    // Get the first three rows of data from the sorted data set.
    let req = get_data_values_request(0, 3, vec![0, 1], default_format_options());

    // Check that sorted values were correctly returned.
    assert_match!(socket_rpc(&socket, req),
                DataExplorerBackendReply::GetDataValuesReply(data) => {
                    // The first three sorted rows should be 10.4, 10.4, and 13.3.
                    assert_eq!(data.columns.len(), 2);
                    assert_eq!(data.columns[0].len(), 3);
                    assert_eq!(data.columns[0][0], ColumnValue::FormattedValue("10.40".to_string()));
                    assert_eq!(data.columns[0][1], ColumnValue::FormattedValue("10.40".to_string()));
                    assert_eq!(data.columns[0][2], ColumnValue::FormattedValue("13.30".to_string()));
        }
    );

    // Row labels should be sorted as well.
    if has_row_names {
        let req = DataExplorerBackendRequest::GetRowLabels(GetRowLabelsParams {
            selection: ArraySelection::SelectIndices(DataSelectionIndices {
                indices: vec![0, 1, 2],
            }),
            format_options: default_format_options(),
        });
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetRowLabelsReply(row_labels) => {
                let labels = row_labels.row_labels;
                assert_eq!(labels[0][0], "Cadillac Fleetwood");
                assert_eq!(labels[0][1], "Lincoln Continental");
                assert_eq!(labels[0][2], "Camaro Z28");
            }
        );
    }

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
    let req = get_data_values_request(0, 3, vec![0, 1], default_format_options());

    // Check that sorted values were correctly returned.
    assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::GetDataValuesReply(data) => {
            assert_eq!(data.columns.len(), 2);
            assert_eq!(data.columns[0][0], ColumnValue::FormattedValue("19.20".to_string()));
            assert_eq!(data.columns[0][1], ColumnValue::FormattedValue("18.70".to_string()));
            assert_eq!(data.columns[0][2], ColumnValue::FormattedValue("17.30".to_string()));
        }
    );
}

#[test]
fn test_basic_mtcars() {
    r_test(|| {
        // --- mtcars ---

        // Test with the regular mtcars data set.
        test_mtcars_sort(
            open_data_explorer(String::from("mtcars")),
            true,
            String::from("mtcars"),
        );
    });
}

#[test]
fn test_tibble_support() {
    r_test(|| {
        let mtcars_tibble = r_parse_eval0("mtcars_tib <- tibble::as_tibble(mtcars)", R_ENVS.global);

        // Now test with a tibble. This might fail if tibble is not installed
        // locally. Just skip the test in that case.
        match mtcars_tibble {
            Ok(_) => {
                test_mtcars_sort(
                    open_data_explorer(String::from("mtcars_tib")),
                    false,
                    String::from("mtcars_tib"),
                );
                r_parse_eval0("rm(mtcars_tib)", R_ENVS.global).unwrap();
            },
            Err(_) => {
                // tibble is not available for testing
            },
        }
    })
}

#[test]
fn test_women_dataset() {
    r_test(|| {
        // --- women ---

        // Open the women data set in the data explorer.
        let socket = open_data_explorer(String::from("women"));

        // Get 2 rows of data from the beginning of the test data set.
        let req = get_data_values_request(0, 2, vec![0, 1], default_format_options());

        // Spot check the data values.
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetDataValuesReply(data) => {
                assert_eq!(data.columns.len(), 2);
                assert_eq!(data.columns[0][0], ColumnValue::FormattedValue("58.00".to_string()));
                assert_eq!(data.columns[0][1], ColumnValue::FormattedValue("59.00".to_string()));
            }
        );

        // Also check row names
        let req = DataExplorerBackendRequest::GetRowLabels(GetRowLabelsParams {
            selection: ArraySelection::SelectIndices(DataSelectionIndices {
                indices: vec![0, 1, 2],
            }),
            format_options: default_format_options(),
        });
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetRowLabelsReply(row_labels) => {
                let labels = row_labels.row_labels;
                assert_eq!(labels[0][0], "1");
                assert_eq!(labels[0][1], "2");
                assert_eq!(labels[0][2], "3");
            }
        );

        // Apply a sort to the data set. We'll sort the first field (height) in
        // descending order.
        let sort_keys = vec![ColumnSortKey {
            column_index: 0,
            ascending: false,
        }];
        let req = DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams {
            sort_keys: sort_keys.clone(),
        });

        // We should get a SetSortColumnsReply back.
        assert_match!(socket_rpc(&socket, req), DataExplorerBackendReply::SetSortColumnsReply() => {});

        // Get the schema of the data set.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            column_indices: vec![0, 1],
        });

        let schema_reply = socket_rpc(&socket, req);
        let schema = match schema_reply {
            DataExplorerBackendReply::GetSchemaReply(schema) => schema,
            _ => panic!("Unexpected reply: {:?}", schema_reply),
        };

        // Next, apply a filter to the data set. We'll filter out all rows where
        // the first field (height) is less than 60.
        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![RowFilter {
                column_schema: schema.columns[0].clone(),
                filter_type: RowFilterType::Compare,
                params: Some(RowFilterParams::Comparison(FilterComparison {
                    op: FilterComparisonOp::Lt,
                    value: "60".to_string(),
                })),
                filter_id: "A11876D6-7CF3-435F-874D-E96892B25C9A".to_string(),
                error_message: None,
                condition: RowFilterCondition::And,
                is_valid: None,
            }],
        });

        // We should get a SetRowFiltersReply back. There are 2 rows where the
        // height is less than 60.
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(false)}
        ) => {
            assert_eq!(num_rows, 2);
        });

        // Get 2 rows of data. These rows should be both sorted and filtered
        // since we have applied both a sort and a filter.
        let req = get_data_values_request(0, 2, vec![0, 1], default_format_options());

        // Spot check the data values.
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetDataValuesReply(data) => {
                // The first column (height) should contain the only two rows
                // where the height is less than 60.
                assert_eq!(data.columns.len(), 2);
                assert_eq!(data.columns[0][0], ColumnValue::FormattedValue("59.00".to_string()));
                assert_eq!(data.columns[0][1], ColumnValue::FormattedValue("58.00".to_string()));
            }
        );
    })
}

#[test]
fn test_matrix_support() {
    r_test(|| {
        // --- volcano (a matrix) ---

        // Open the volcano data set in the data explorer. This data set is a matrix.
        let socket = open_data_explorer(String::from("volcano"));

        // Get the schema for the test data set.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            column_indices: (0..61).collect_vec(),
        });

        // Check that we got the right number of columns.
        let schema_reply = socket_rpc(&socket, req);
        let schema = match schema_reply {
            DataExplorerBackendReply::GetSchemaReply(schema) => schema,
            _ => panic!("Unexpected reply: {:?}", schema_reply),
        };
        assert_eq!(schema.columns.len(), 61);

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
        let req = get_data_values_request(0, 4, vec![0, 1], default_format_options());

        // Check the data values.
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetDataValuesReply(data) => {
                assert_eq!(data.columns.len(), 2);
                assert_eq!(data.columns[0][0], ColumnValue::FormattedValue("97.00".to_string()));
                assert_eq!(data.columns[0][1], ColumnValue::FormattedValue("97.00".to_string()));
                assert_eq!(data.columns[0][2], ColumnValue::FormattedValue("98.00".to_string()));
                assert_eq!(data.columns[0][3], ColumnValue::FormattedValue("98.00".to_string()));
            }
        );

        // Next, apply a filter to the data set. We'll filter out all rows where
        // the first column is less than 100.
        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![RowFilter {
                column_schema: schema.columns[0].clone(),
                filter_type: RowFilterType::Compare,
                params: Some(RowFilterParams::Comparison(FilterComparison {
                    op: FilterComparisonOp::Lt,
                    value: "100".to_string(),
                })),
                filter_id: "F5D5FE28-04D9-4010-8C77-84094D9B8E2C".to_string(),
                condition: RowFilterCondition::And,
                error_message: None,
                is_valid: None,
            }],
        });

        // We should get a SetRowFiltersReply back. There are 8 rows where the
        // first column of the matrix is less than 100.
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(false)}
        ) => {
            assert_eq!(num_rows, 8);
        });
    })
}

#[test]
fn test_data_table_support() {
    r_test(|| {
        // --- mtcars (as a data.table) ---

        let mtcars_data_table =
            r_parse_eval0("mtcars_dt <- data.table::data.table(mtcars)", R_ENVS.global);

        // Now test with a data.table. This might fail if data.table is not installed
        // locally. Just skip the test in that case.
        match mtcars_data_table {
            Ok(_) => {
                test_mtcars_sort(
                    open_data_explorer(String::from("mtcars_dt")),
                    false,
                    String::from("mtcars_dt"),
                );
                r_parse_eval0("rm(mtcars_dt)", R_ENVS.global).unwrap();
            },
            Err(_) => (),
        }
    })
}

#[test]
fn test_null_counts() {
    r_test(|| {
        // --- null count ---

        // Create a data frame with the Fibonacci sequence, including some NA values
        // where a number in the sequence has been omitted.
        let socket = open_data_explorer_from_expression(
            "fibo <- data.frame(col = c(1, NA, 2, 3, 5, NA, 13, 21, NA))",
            None,
        )
        .unwrap();

        // Get the schema of the data set.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            column_indices: vec![0],
        });

        let schema_reply = socket_rpc(&socket, req);
        let schema = match schema_reply {
            DataExplorerBackendReply::GetSchemaReply(schema) => schema,
            _ => panic!("Unexpected reply: {:?}", schema_reply),
        };

        // Ask for a count of nulls in the first column.
        let req = DataExplorerBackendRequest::GetColumnProfiles(GetColumnProfilesParams {
            profiles: vec![ColumnProfileRequest {
                column_index: 0,
                profiles: vec![ColumnProfileSpec {
                    profile_type: ColumnProfileType::NullCount,
                    params: None,
                }],
            }],
            format_options: default_format_options(),
        });

        assert_match!(socket_rpc(&socket, req),
           DataExplorerBackendReply::GetColumnProfilesReply(data) => {
               // We asked for the null count of the first column, which has 3 NA values.
               assert!(data.len() == 1);
               assert_eq!(data[0].null_count, Some(3));
           }
        );

        // Next, apply a filter to the data set. Filter out all empty rows.
        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![RowFilter {
                column_schema: schema.columns[0].clone(),
                filter_type: RowFilterType::NotNull,
                filter_id: "048D4D03-A7B5-4825-BEB1-769B70DE38A6".to_string(),
                condition: RowFilterCondition::And,
                is_valid: None,
                error_message: None,
                params: None,
            }],
        });

        // We should get a SetRowFiltersReply back. There are 6 rows where the
        // first column is not NA.
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(false) }
        ) => {
            assert_eq!(num_rows, 6);
        });

        // Ask for a count of nulls in the first column again. Since a filter
        // has been applied, the null count should be 0.
        let req = DataExplorerBackendRequest::GetColumnProfiles(GetColumnProfilesParams {
            profiles: vec![ColumnProfileRequest {
                column_index: 0,
                profiles: vec![ColumnProfileSpec {
                    profile_type: ColumnProfileType::NullCount,
                    params: None,
                }],
            }],
            format_options: default_format_options(),
        });

        assert_match!(socket_rpc(&socket, req),
           DataExplorerBackendReply::GetColumnProfilesReply(data) => {
               // We asked for the null count of the first column, which has no
                // NA values after the filter.
               assert!(data.len() == 1);
               assert_eq!(data[0].null_count, Some(0));
           }
        );

        // Let's look at JUST the empty rows.
        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![RowFilter {
                column_schema: schema.columns[0].clone(),
                filter_type: RowFilterType::IsNull,
                filter_id: "87E2E016-C853-4928-8914-8774125E3C87".to_string(),
                condition: RowFilterCondition::And,
                is_valid: None,
                params: None,
                error_message: None,
            }],
        });

        // We should get a SetRowFiltersReply back. There are 3 rows where the
        // first field has a missing value.
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(false)}
        ) => {
            assert_eq!(num_rows, 3);
        });
    })
}

#[test]
fn test_summary_stats() {
    r_test(|| {
        // --- summary stats ---

        // Create a data frame with some numbers, characters and booleans to test
        // summary statistics.
        r_parse_eval0(
            "df <- data.frame(num = c(1, 2, 3, NA), char = c('a', 'a', '', NA), bool = c(TRUE, TRUE, FALSE, NA))",
            R_ENVS.global,
        )
        .unwrap();

        // Open the fibo data set in the data explorer.
        let socket = open_data_explorer(String::from("df"));

        // Ask for summary stats for the columns
        let req = DataExplorerBackendRequest::GetColumnProfiles(GetColumnProfilesParams {
            profiles: (0..3)
                .map(|i| ColumnProfileRequest {
                    column_index: i,
                    profiles: vec![ColumnProfileSpec {
                        profile_type: ColumnProfileType::SummaryStats,
                        params: None,
                    }],
                })
                .collect(),
            format_options: default_format_options(),
        });

        assert_match!(socket_rpc(&socket, req),
           DataExplorerBackendReply::GetColumnProfilesReply(data) => {
                // We asked for summary stats for all 3 columns
                assert!(data.len() == 3);

                // The first column is numeric and has 3 non-NA values.
                assert!(data[0].summary_stats.is_some());
                let number_stats = data[0].summary_stats.clone().unwrap().number_stats;
                assert!(number_stats.is_some());
                let number_stats = number_stats.unwrap();
                assert_eq!(number_stats, SummaryStatsNumber {
                    min_value: Some(String::from("1.00")),
                    max_value: Some(String::from("3.00")),
                    mean: Some(String::from("2.00")),
                    median: Some(String::from("2.00")),
                    stdev: Some(String::from("1.00")),
                });

                // The second column is a character column
                assert!(data[1].summary_stats.is_some());
                let string_stats = data[1].summary_stats.clone().unwrap().string_stats;
                assert!(string_stats.is_some());
                let string_stats = string_stats.unwrap();
                assert_eq!(string_stats, SummaryStatsString {
                    num_empty: 1,
                    num_unique: 3, // NA's are counted as unique values
                });

                // The third column is boolean
                assert!(data[2].summary_stats.is_some());
                let boolean_stats = data[2].summary_stats.clone().unwrap().boolean_stats;
                assert!(boolean_stats.is_some());
                let boolean_stats = boolean_stats.unwrap();
                assert_eq!(boolean_stats, SummaryStatsBoolean {
                    true_count: 2,
                    false_count: 1,
                });

           }
        );
    })
}

#[test]
fn test_search_filters() {
    r_test(|| {
        // --- search filters ---

        // Create a data frame with a bunch of words to use for regex testing.
        r_parse_eval0(
            r#"words <- data.frame(text = c(
                "lambent",
                "incandescent",
                "that will be $10.26",
                "pi is 3.14159",
                "",
                "weasel",
                "refrigerator"
            ))"#,
            R_ENVS.global,
        )
        .unwrap();

        // Open the words data set in the data explorer.
        let socket = open_data_explorer(String::from("words"));

        // Get the schema of the data set.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            column_indices: vec![0],
        });

        let schema_reply = socket_rpc(&socket, req);
        let schema = match schema_reply {
            DataExplorerBackendReply::GetSchemaReply(schema) => schema,
            _ => panic!("Unexpected reply: {:?}", schema_reply),
        };

        // Next, apply a filter to the data set. Check for rows that contain the
        // text ".".
        let dot_filter = RowFilter {
            column_schema: schema.columns[0].clone(),
            filter_type: RowFilterType::Search,
            filter_id: "A58A4497-29E0-4407-BC25-67FEF73F6224".to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            params: Some(RowFilterParams::TextSearch(FilterTextSearch {
                case_sensitive: false,
                search_type: TextSearchType::Contains,
                term: ".".to_string(),
            })),
            error_message: None,
        };
        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![dot_filter.clone()],
        });

        // We should get a SetRowFiltersReply back. There are 2 rows where
        // the text contains ".".
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(false)}
        ) => {
            assert_eq!(num_rows, 2);
        });

        // Combine this with an OR filter that checks for rows that end in
        // 'ent'.
        let ent_filter = RowFilter {
            column_schema: schema.columns[0].clone(),
            filter_type: RowFilterType::Search,
            filter_id: "4BA46699-EF41-4FA8-A927-C8CD88520D6E".to_string(),
            condition: RowFilterCondition::Or,
            is_valid: None,
            params: Some(RowFilterParams::TextSearch(FilterTextSearch {
                case_sensitive: false,
                search_type: TextSearchType::EndsWith,
                term: "ent".to_string(),
            })),
            error_message: None,
        };

        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![dot_filter, ent_filter],
        });

        // We should get a SetRowFiltersReply back. There are 4 rows where
        // the text either contains "." OR ends in "ent".
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(false) }
        ) => {
            assert_eq!(num_rows, 4);
        });

        // Create a filter for empty values.
        let empty_filter = RowFilter {
            column_schema: schema.columns[0].clone(),
            filter_type: RowFilterType::IsEmpty,
            filter_id: "3F032747-4667-40CB-9013-AA659AE37F1C".to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            params: None,
            error_message: None,
        };

        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![empty_filter],
        });

        // We should get a SetRowFiltersReply back. There's 1 row with an empty
        // value.
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(false) }
        ) => {
            assert_eq!(num_rows, 1);
        });

        // Check the table state; at this point we should have 1 row from 7 total.
        let req = DataExplorerBackendRequest::GetState;
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetStateReply(state) => {
                assert_eq!(state.table_shape.num_rows, 1);
                assert_eq!(state.table_unfiltered_shape.num_rows, 7);
            }
        );

        // --- invalid filters ---

        // Create a data frame with a bunch of dates.
        r_parse_eval0(
            r#"test_dates <- data.frame(date = as.POSIXct(c(
                    "2024-01-01 01:00:00",
                    "2024-01-02 02:00:00",
                    "2024-01-03 03:00:00"))
            )"#,
            R_ENVS.global,
        )
        .unwrap();

        // Open the dates data set in the data explorer.
        let socket = open_data_explorer(String::from("test_dates"));

        // Get the schema of the data set.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            column_indices: vec![0],
        });

        let schema_reply = socket_rpc(&socket, req);
        let schema = match schema_reply {
            DataExplorerBackendReply::GetSchemaReply(schema) => schema,
            _ => panic!("Unexpected reply: {:?}", schema_reply),
        };

        // Next, apply a filter to the data set. Check for rows that are greater than
        // "marshmallows". This is an invalid filter because the column is a date.
        let year_filter = RowFilter {
            column_schema: schema.columns[0].clone(),
            filter_type: RowFilterType::Compare,
            filter_id: "0DB2F23D-B299-4068-B8D5-A2B513A93330".to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            params: Some(RowFilterParams::Comparison(FilterComparison {
                op: FilterComparisonOp::Gt,
                value: "marshmallows".to_string(),
            })),
            error_message: None,
        };
        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![year_filter.clone()],
        });

        // We should get a SetRowFiltersReply back. Because the filter is invalid,
        // the number of selected rows should be 3 (all the rows in the data set)
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(true)}
        ) => {
            assert_eq!(num_rows, 3);
        });

        // We also want to make sure that invalid filters are marked along with their
        // error messages.
        let req = DataExplorerBackendRequest::GetState;
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetStateReply(state) => {
                assert_eq!(state.row_filters[0].is_valid, Some(false));
                assert!(state.row_filters[0].error_message.is_some());
            }
        );

        // --- boolean filters ---

        // Create a data frame with a series of boolean values.
        r_parse_eval0(
            r#"test_bools <- data.frame(bool = c(
                    TRUE,
                    TRUE,
                    FALSE,
                    NA,
                    TRUE,
                    FALSE
            ))"#,
            R_ENVS.global,
        )
        .unwrap();

        // Open the data set in the data explorer.
        let socket = open_data_explorer(String::from("test_bools"));

        // Get the schema of the data set.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            column_indices: vec![0],
        });

        let schema_reply = socket_rpc(&socket, req);
        let schema = match schema_reply {
            DataExplorerBackendReply::GetSchemaReply(schema) => schema,
            _ => panic!("Unexpected reply: {:?}", schema_reply),
        };

        // Next, apply a filter to the data set. Check for rows that are TRUE.
        let true_filter = RowFilter {
            column_schema: schema.columns[0].clone(),
            filter_type: RowFilterType::IsTrue,
            filter_id: "16B3E3E7-44D0-4003-B6BD-46EE0629F067".to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            params: None,
            error_message: None,
        };
        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![true_filter.clone()],
        });

        // We should get a SetRowFiltersReply back. There are 3 rows where the
        // value is TRUE.
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(false)}
        ) => {
            assert_eq!(num_rows, 3);
        });

        // ---- formatting of list columns ----
        let val = r_parse_eval0(
            r#"list_cols <- tibble::tibble(
                list_col = list(c(1,2,3,4), tibble::tibble(x = 1, b = 2), matrix(1:4, nrow = 2), c(TRUE, FALSE)),
                list_col_class = vctrs::list_of(1,2,3, 4)
            )"#,
            R_ENVS.global,
        );

        match val {
            Ok(_) => {
                // Open the data set in the data explorer.
                let socket = open_data_explorer(String::from("list_cols"));

                // Get the values from the first column again. Because a sort is applied,
                // the new value we wrote should be at the end.
                let req = get_data_values_request(0, 4, vec![0, 1], default_format_options());
                assert_match!(socket_rpc(&socket, req),
                    DataExplorerBackendReply::GetDataValuesReply(data) => {
                        assert_eq!(data.columns.len(), 2);
                        assert_eq!(data.columns[0][0], ColumnValue::FormattedValue("<numeric [4]>".to_string()));
                        assert_eq!(data.columns[0][1], ColumnValue::FormattedValue("<tbl_df [1 x 2]>".to_string()));
                        assert_eq!(data.columns[0][2], ColumnValue::FormattedValue("<matrix [2 x 2]>".to_string()));
                        assert_eq!(data.columns[0][3], ColumnValue::FormattedValue("<logical [2]>".to_string()));

                        assert_eq!(data.columns[1][0], ColumnValue::FormattedValue("1".to_string()));
                        assert_eq!(data.columns[1][1], ColumnValue::FormattedValue("2".to_string()));
                        assert_eq!(data.columns[1][2], ColumnValue::FormattedValue("3".to_string()));
                        assert_eq!(data.columns[1][3], ColumnValue::FormattedValue("4".to_string()));
                    }
                );
            },
            Err(_) => {
                // Skip test if tibble/vctrs not installed
            },
        }
    });
}

#[test]
fn test_live_updates() {
    r_test(|| {
        let socket = open_data_explorer_from_expression(
            "x <- data.frame(y = c(3, 2, 1), z = c(4, 5, 6))",
            Some("x"),
        )
        .unwrap();

        // Make a data-level change to the data set.
        r_parse_eval0("x[1, 1] <- 0", R_ENVS.global).unwrap();

        // Emit a console prompt event; this should tickle the data explorer to
        // check for changes.
        EVENTS.console_prompt.emit(());

        // Wait for an update event to arrive
        assert_match!(socket.socket.outgoing_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
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
        let req = get_data_values_request(0, 3, vec![0], default_format_options());
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetDataValuesReply(data) => {
                assert_eq!(data.columns.len(), 1);
                assert_eq!(data.columns[0][0], ColumnValue::FormattedValue("0.00".to_string()));
                assert_eq!(data.columns[0][1], ColumnValue::FormattedValue("1.00".to_string()));
                assert_eq!(data.columns[0][2], ColumnValue::FormattedValue("2.00".to_string()));
            }
        );

        // Make another data-level change to the data set.
        r_parse_eval0("x[1, 1] <- 3", R_ENVS.global).unwrap();

        // Emit a console prompt event; this should tickle the data explorer to
        // check for changes.
        EVENTS.console_prompt.emit(());

        // Wait for an update event to arrive
        assert_match!(socket.socket.outgoing_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            CommMsg::Data(value) => {
                // Make sure it's a data update event.
                assert_match!(serde_json::from_value::<DataExplorerFrontendEvent>(value).unwrap(),
                    DataExplorerFrontendEvent::DataUpdate
                );
        });

        // Get the values from the first column again. Because a sort is applied,
        // the new value we wrote should be at the end.
        let req = get_data_values_request(0, 3, vec![0], default_format_options());
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetDataValuesReply(data) => {
                assert_eq!(data.columns.len(), 1);
                assert_eq!(data.columns[0][0], ColumnValue::FormattedValue("1.00".to_string()));
                assert_eq!(data.columns[0][1], ColumnValue::FormattedValue("2.00".to_string()));
                assert_eq!(data.columns[0][2], ColumnValue::FormattedValue("3.00".to_string()));
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
        assert_match!(socket.socket.outgoing_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            CommMsg::Data(value) => {
                // Make sure it's schema update event.
                assert_match!(serde_json::from_value::<DataExplorerFrontendEvent>(value).unwrap(),
                    DataExplorerFrontendEvent::SchemaUpdate);
        });

        // Get the schema again to make sure it updated. We added a new column, so
        // we should get 3 columns back.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            column_indices: vec![0, 1, 2],
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
        assert_match!(socket.socket.outgoing_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            CommMsg::Close => {}
        );
    })
}

#[test]
fn test_boolean_filters() {
    r_test(|| {
        // --- boolean filters ---

        // Create a data frame with a series of boolean values.
        r_parse_eval0(
            r#"test_bools <- data.frame(bool = c(
                    TRUE,
                    TRUE,
                    FALSE,
                    NA,
                    TRUE,
                    FALSE
            ))"#,
            R_ENVS.global,
        )
        .unwrap();

        // Open the data set in the data explorer.
        let socket = open_data_explorer(String::from("test_bools"));

        // Get the schema of the data set.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            column_indices: vec![0],
        });

        let schema_reply = socket_rpc(&socket, req);
        let schema = match schema_reply {
            DataExplorerBackendReply::GetSchemaReply(schema) => schema,
            _ => panic!("Unexpected reply: {:?}", schema_reply),
        };

        // Next, apply a filter to the data set. Check for rows that are TRUE.
        let true_filter = RowFilter {
            column_schema: schema.columns[0].clone(),
            filter_type: RowFilterType::IsTrue,
            filter_id: "16B3E3E7-44D0-4003-B6BD-46EE0629F067".to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            params: None,
            error_message: None,
        };
        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![true_filter.clone()],
        });

        // We should get a SetRowFiltersReply back. There are 3 rows where the
        // value is TRUE.
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(false)}
        ) => {
            assert_eq!(num_rows, 3);
        });
    })
}

#[test]
fn test_invalid_filters() {
    r_test(|| {
        // --- invalid filters ---

        // Create a data frame with a bunch of dates.
        let socket = open_data_explorer_from_expression(
            r#"test_dates <- data.frame(date = as.POSIXct(c(
                    "2024-01-01 01:00:00",
                    "2024-01-02 02:00:00",
                    "2024-01-03 03:00:00"))
                    )"#,
            None,
        )
        .unwrap();

        // Get the schema of the data set.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            column_indices: vec![0],
        });

        let schema_reply = socket_rpc(&socket, req);
        let schema = match schema_reply {
            DataExplorerBackendReply::GetSchemaReply(schema) => schema,
            _ => panic!("Unexpected reply: {:?}", schema_reply),
        };

        // Next, apply a filter to the data set. Check for rows that are greater than
        // "marshmallows". This is an invalid filter because the column is a date.
        let year_filter = RowFilter {
            column_schema: schema.columns[0].clone(),
            filter_type: RowFilterType::Compare,
            filter_id: "0DB2F23D-B299-4068-B8D5-A2B513A93330".to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            params: Some(RowFilterParams::Comparison(FilterComparison {
                op: FilterComparisonOp::Gt,
                value: "marshmallows".to_string(),
            })),
            error_message: None,
        };
        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![year_filter.clone()],
        });

        // We should get a SetRowFiltersReply back. Because the filter is invalid,
        // the number of selected rows should be 3 (all the rows in the data set)
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(true)}
        ) => {
            assert_eq!(num_rows, 3);
        });

        // We also want to make sure that invalid filters are marked along with their
        // error messages.
        let req = DataExplorerBackendRequest::GetState;
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetStateReply(state) => {
                assert_eq!(state.row_filters[0].is_valid, Some(false));
                assert!(state.row_filters[0].error_message.is_some());
            }
        );
    })
}

// Tests that invalid filters are preserved after a live update that removes the column
// Refer to https://github.com/posit-dev/positron/issues/3141 for more info.
#[test]
fn test_invalid_filters_preserved() {
    r_test(|| {
        let socket = open_data_explorer_from_expression(
            r#"test_df <- data.frame(x = c('','a', 'b'), y = c(1, 2, 3))"#,
            Some("test_df"),
        )
        .unwrap();

        // Get the schema of the data set.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            column_indices: vec![0],
        });

        let schema_reply = socket_rpc(&socket, req);
        let schema = match schema_reply {
            DataExplorerBackendReply::GetSchemaReply(schema) => schema,
            _ => panic!("Unexpected reply: {:?}", schema_reply),
        };

        // Next, apply a filter to the data set. Check for rows that are greater than
        // "marshmallows". This is an invalid filter because the column is a date.
        let x_is_empty = RowFilter {
            column_schema: schema.columns[0].clone(),
            filter_type: RowFilterType::IsEmpty,
            filter_id: "0DB2F23D-B299-4068-B8D5-A2B513A93330".to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            params: None,
            error_message: None,
        };

        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![x_is_empty.clone()],
        });

        // We should get a SetRowFiltersReply back and we should get a single row
        assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(false)}
        ) => {
            assert_eq!(num_rows, 1);
        });

        // Now let's update the data frame removing the 'x' column, the filter should
        // now be invalid.
        r_parse_eval0("test_df$x <- NULL", R_ENVS.global).unwrap();

        // Emit a console prompt event; this should tickle the data explorer to
        // check for changes.
        EVENTS.console_prompt.emit(());

        // Wait for an update event to arrive
        assert_match!(socket.socket.outgoing_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            CommMsg::Data(value) => {
                // Make sure it's a data update event.
                assert_match!(serde_json::from_value::<DataExplorerFrontendEvent>(value).unwrap(),
                    DataExplorerFrontendEvent::SchemaUpdate
                );
        });

        // Check the backend state. The filter should be marked invalid and have an error message.
        let req = DataExplorerBackendRequest::GetState;
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetStateReply(state) => {
                assert_eq!(state.row_filters[0].is_valid, Some(false));
                assert!(state.row_filters[0].error_message.is_some());
                assert_eq!(state.table_shape.num_rows, 3);
            }
        );

        // we now re-assign the column to make the filter valid again and see if it's re-applied
        r_parse_eval0("test_df$x <- c('','a', 'b')", R_ENVS.global).unwrap();
        // Emit a console prompt event; this should tickle the data explorer to
        // check for changes.
        EVENTS.console_prompt.emit(());
        // Wait for an update event to arrive
        assert_match!(socket.socket.outgoing_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            CommMsg::Data(value) => {
                // Make sure it's a data update event.
                assert_match!(serde_json::from_value::<DataExplorerFrontendEvent>(value).unwrap(),
                    DataExplorerFrontendEvent::SchemaUpdate
                );
        });
        // Check the backend state. The filter should be marked valid
        let req = DataExplorerBackendRequest::GetState;
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetStateReply(state) => {
                assert_eq!(state.row_filters[0].is_valid, Some(true));
                assert!(state.row_filters[0].error_message.is_none());
                assert_eq!(state.table_shape.num_rows, 1);
            }
        );

        // now make the filter invalid because of the data type has changed
        r_parse_eval0("test_df$x <- c(1, 2, 3)", R_ENVS.global).unwrap();
        // Emit a console prompt event; this should tickle the data explorer to
        // check for changes.
        EVENTS.console_prompt.emit(());
        // Wait for an update event to arrive
        assert_match!(socket.socket.outgoing_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            CommMsg::Data(value) => {
                // Make sure it's a data update event.
                assert_match!(serde_json::from_value::<DataExplorerFrontendEvent>(value).unwrap(),
                    DataExplorerFrontendEvent::SchemaUpdate
                );
        });
        // Check the backend state. The filter should be marked valid
        let req = DataExplorerBackendRequest::GetState;
        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetStateReply(state) => {
                assert_eq!(state.row_filters[0].is_valid, Some(false));
                assert!(state.row_filters[0].error_message.is_some());
                assert_eq!(state.table_shape.num_rows, 3);
            }
        );

        r_parse_eval0("rm(test_df)", R_ENVS.global).unwrap();
    });
}

#[test]
fn test_data_explorer_special_values() {
    r_test(|| {
        let code = "x <- tibble::tibble(
            a = c(1, NA, NaN, Inf, -Inf),
            b = c('a', 'b', 'c', 'd', NA),
            c = c(TRUE, FALSE, NA, NA, NA),
            d = c(1:4, NA),
            e = c(complex(4), NA),
            f = list(NULL, list(1,2,3), list(4,5,6), list(7,8,9), list(10,11,12))
        )";

        let socket = match open_data_explorer_from_expression(code, None) {
            Ok(socket) => socket,
            Err(_) => return, // Skip test if tibble is not installed
        };

        let req = get_data_values_request(0, 5, vec![0, 1, 2, 3, 4, 5], default_format_options());

        assert_match!(socket_rpc(&socket, req),
            DataExplorerBackendReply::GetDataValuesReply(data) => {
                assert_eq!(data.columns.len(), 6);

                assert_eq!(data.columns[0][0], ColumnValue::FormattedValue("1.00".to_string()));
                assert_eq!(data.columns[0][1], ColumnValue::SpecialValueCode(1));
                assert_eq!(data.columns[0][2], ColumnValue::SpecialValueCode(2));
                assert_eq!(data.columns[0][3], ColumnValue::SpecialValueCode(10));
                assert_eq!(data.columns[0][4], ColumnValue::SpecialValueCode(11));

                assert_eq!(data.columns[1][4], ColumnValue::SpecialValueCode(1));
                assert_eq!(data.columns[2][4], ColumnValue::SpecialValueCode(1));
                assert_eq!(data.columns[3][4], ColumnValue::SpecialValueCode(1));
                assert_eq!(data.columns[4][4], ColumnValue::SpecialValueCode(1));

                assert_eq!(data.columns[5][0], ColumnValue::SpecialValueCode(0));
            }

        );
        r_parse_eval0("rm(x)", R_ENVS.global).unwrap();
    });
}

// The main exporting logic is tested in the data_exporter module. This test
// is mainly an integration test to check if the data explorer can correctly
// work with sorting/filtering the data and then exporting it.
#[test]
fn test_export_data() {
    r_test(|| {
        let socket = open_data_explorer_from_expression(
            r#"
            data.frame(
                a = c(1, 3, 2),
                b = c('a', 'b', 'c'),
                c = c(TRUE, FALSE, TRUE)
            )
        "#,
            None,
        )
        .unwrap();

        let selection_req =
            DataExplorerBackendRequest::ExportDataSelection(ExportDataSelectionParams {
                format: ExportFormat::Csv,
                selection: TableSelection {
                    kind: TableSelectionKind::SingleCell,
                    selection: Selection::SingleCell(DataSelectionSingleCell {
                        row_index: 1,
                        column_index: 1,
                    }),
                },
            });

        assert_match!(socket_rpc(&socket, selection_req.clone()),
            DataExplorerBackendReply::ExportDataSelectionReply(ExportedData {format, data}) => {
                assert_eq!(data, "b".to_string());
                assert_eq!(format, ExportFormat::Csv);
            }
        );

        // sort the data frame
        let sort_req = DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams {
            sort_keys: vec![ColumnSortKey {
                column_index: 0,
                ascending: false,
            }],
        });
        socket_rpc(&socket, sort_req);

        assert_match!(socket_rpc(&socket, selection_req.clone()),
            DataExplorerBackendReply::ExportDataSelectionReply(ExportedData {format, data}) => {
                assert_eq!(data, "c".to_string());
                assert_eq!(format, ExportFormat::Csv);
            }
        );

        // now filter the data frame
        let schemas_req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            column_indices: vec![0, 1, 2],
        });
        let schema = match socket_rpc(&socket, schemas_req) {
            DataExplorerBackendReply::GetSchemaReply(schema) => schema,
            _ => panic!("Unexpected reply"),
        };

        let filter_req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![RowFilter {
                column_schema: schema.columns[2].clone(),
                filter_type: RowFilterType::IsTrue,
                filter_id: "1".to_string(),
                condition: RowFilterCondition::And,
                is_valid: None,
                params: None,
                error_message: None,
            }],
        });
        socket_rpc(&socket, filter_req);

        assert_match!(socket_rpc(&socket, selection_req.clone()),
            DataExplorerBackendReply::ExportDataSelectionReply(ExportedData {format, data}) => {
                assert_eq!(data, "a".to_string());
                assert_eq!(format, ExportFormat::Csv);
            }
        );
    })
}

// Tests that filters and sorts are reapplied to new data after a Data Update event.
// A regression test for https://github.com/posit-dev/positron/issues/4170
#[test]
fn test_update_data_filters_reapplied() {
    r_test(|| {
        let socket = open_data_explorer_from_expression(
            r#"
            x <- data.frame(
                a = c(3, 3, 3, 1),
                b = c('a', 'b', 'c', 'd')
            )
        "#,
            Some("x"),
        )
        .unwrap();

        // Get the schema of the data set.
        let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
            column_indices: vec![0],
        });

        let schema_reply = socket_rpc(&socket, req);
        let schema = match schema_reply {
            DataExplorerBackendReply::GetSchemaReply(schema) => schema,
            _ => panic!("Unexpected reply: {:?}", schema_reply),
        };

        // Apply filter by the `a` columns. Expecting to get 3 rows larger than 1.
        let x_gt_1 = RowFilter {
            column_schema: schema.columns[0].clone(),
            filter_type: RowFilterType::Compare,
            filter_id: "0DB2F23D-B299-4068-B8D5-A2B513A93330".to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            params: Some(RowFilterParams::Comparison(FilterComparison {
                op: FilterComparisonOp::Gt,
                value: "1".to_string(),
            })),
            error_message: None,
        };
        let req = DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams {
            filters: vec![x_gt_1.clone()],
        });
        // Set filters should display 3 rows that are greater than 1.
        assert_match!(socket_rpc(&socket, req.clone()),
        DataExplorerBackendReply::SetRowFiltersReply(
            FilterResult { selected_num_rows: num_rows, had_errors: Some(false)}
        ) => {
            assert_eq!(num_rows, 3);
        });

        // Also add a sorting to check that data will be sorted in the correct way
        // after the data update.
        // Create a request to sort the data set by the 'mpg' column.
        let sort_keys = vec![ColumnSortKey {
            column_index: 0,
            ascending: true,
        }];
        let req = DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams {
            sort_keys: sort_keys.clone(),
        });
        // We should get a SetSortColumnsReply back.
        assert_match!(socket_rpc(&socket, req), DataExplorerBackendReply::SetSortColumnsReply() => {});

        // Check the number of rows when using the GetData method
        let expect_get_data_rows = |n, values| {
            // Getting data out of the data explorer should have the filters applied
            let req = get_data_values_request(0, 5, vec![0, 1], default_format_options());

            // Check that we got the right columns and row labels.
            assert_match!(socket_rpc(&socket, req),
                DataExplorerBackendReply::GetDataValuesReply(data) => {
                    assert_eq!(data.columns[0].len(), n);
                    assert_eq!(data.columns[1], values);
                }
            );
        };

        // GetData should also display 2 rows only
        expect_get_data_rows(3, vec![
            ColumnValue::FormattedValue("a".to_string()),
            ColumnValue::FormattedValue("b".to_string()),
            ColumnValue::FormattedValue("c".to_string()),
        ]);

        // Now make the filter invalid because of the data type has changed
        r_parse_eval0("x$a <- c(3, 2, 1, 1)", R_ENVS.global).unwrap();
        // Emit a console prompt event; this should tickle the data explorer to
        // check for changes.
        EVENTS.console_prompt.emit(());

        // Wait for an update event to arrive
        // Since only data changed, we expect a Data Update Event
        assert_match!(socket.socket.outgoing_rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            CommMsg::Data(value) => {
                // Make sure it's a data update event.
                assert_match!(serde_json::from_value::<DataExplorerFrontendEvent>(value).unwrap(),
                    DataExplorerFrontendEvent::DataUpdate
                );
        });

        // We now expect 2 rows when getting data
        // It should also be sorted differently
        expect_get_data_rows(2, vec![
            ColumnValue::FormattedValue("b".to_string()),
            ColumnValue::FormattedValue("a".to_string()),
        ]);
    });
}

#[test]
fn test_get_data_values_by_indices() {
    r_test(|| {
        let socket = open_data_explorer_from_expression(
            "data.frame(x = c(1:10), y = letters[1:10], z = seq(0,1, length.out = 10))",
            None,
        )
        .unwrap();

        let make_req = |column_indices: Vec<i64>, row_indices: Vec<i64>| {
            let columns = column_indices
                .into_iter()
                .map(|column_index| ColumnSelection {
                    column_index,
                    spec: ArraySelection::SelectIndices(DataSelectionIndices {
                        indices: row_indices.clone(),
                    }),
                })
                .collect();

            DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
                columns,
                format_options: default_format_options(),
            })
        };

        let expect_get_data_values = |column_indices, row_indices, results: Vec<Vec<&str>>| {
            assert_match!(socket_rpc(&socket, make_req(column_indices, row_indices)),
                DataExplorerBackendReply::GetDataValuesReply(data) => {
                    for (i, value) in enumerate(data.columns.iter()) {
                        let formatted_results: Vec<Vec<ColumnValue>> = results.clone().into_iter().map(|inner| {
                            inner.into_iter().map(|v| ColumnValue::FormattedValue(v.to_string())).collect()
                        }).collect();
                        assert_eq!(*value, formatted_results[i]);
                    }
                }
            );
        };

        expect_get_data_values(vec![0], vec![0, 9], vec![vec!["1", "10"]]);
        expect_get_data_values(vec![1], vec![2, 4], vec![vec!["c", "e"]]);
        expect_get_data_values(vec![2], vec![0, 9], vec![vec!["0.00", "1.00"]]);
        expect_get_data_values(vec![2], vec![0, 10], vec![vec!["0.00"]]); // Ignore oout of bounds
    })
}

#[test]
fn test_histogram() {
    r_test(|| {
        let socket =
            open_data_explorer_from_expression("data.frame(x = rep(1:10, 10:1))", None).unwrap();

        let make_histogram_req = |column_index, method, num_bins, quantiles| {
            DataExplorerBackendRequest::GetColumnProfiles(GetColumnProfilesParams {
                profiles: vec![ColumnProfileRequest {
                    column_index,
                    profiles: vec![ColumnProfileSpec {
                        profile_type: ColumnProfileType::Histogram,
                        params: Some(ColumnProfileParams::Histogram(ColumnHistogramParams {
                            method,
                            num_bins,
                            quantiles,
                        })),
                    }],
                }],
                format_options: default_format_options(),
            })
        };

        let req = make_histogram_req(0, ColumnHistogramParamsMethod::Fixed, Some(10), None);

        assert_match!(socket_rpc(&socket, req), DataExplorerBackendReply::GetColumnProfilesReply(profiles) => {
            let histogram = profiles[0].histogram.clone().unwrap();
            assert_eq!(histogram, ColumnHistogram {
                bin_edges: format_string(r_parse_eval0("c(1:10)", R_ENVS.global).unwrap().sexp, &default_format_options()),
                bin_counts: vec![19, 8, 7, 6, 5, 4, 3, 2, 1], // Pretty bind edges unite the first two intervals
                quantiles: vec![],
            });
        });
    })
}

#[test]
fn test_frequency_table() {
    r_test(|| {
        let socket =
            open_data_explorer_from_expression("data.frame(x = rep(letters[1:10], 10:1))", None)
                .unwrap();

        let make_freq_table_req = |column_index, limit| {
            DataExplorerBackendRequest::GetColumnProfiles(GetColumnProfilesParams {
                profiles: vec![ColumnProfileRequest {
                    column_index,
                    profiles: vec![ColumnProfileSpec {
                        profile_type: ColumnProfileType::FrequencyTable,
                        params: Some(ColumnProfileParams::FrequencyTable(
                            ColumnFrequencyTableParams { limit },
                        )),
                    }],
                }],
                format_options: default_format_options(),
            })
        };

        let req = make_freq_table_req(0, 5);

        assert_match!(socket_rpc(&socket, req), DataExplorerBackendReply::GetColumnProfilesReply(profiles) => {
            let freq_table = profiles[0].frequency_table.clone().unwrap();
            assert_eq!(freq_table, ColumnFrequencyTable {
                values: format_string(r_parse_eval0("letters[1:5]", R_ENVS.global).unwrap().sexp, &default_format_options()),
                counts: vec![10, 9, 8, 7, 6],
                other_count: Some(5 + 4 + 3 + 2 + 1)
            });
        });
    })
}
