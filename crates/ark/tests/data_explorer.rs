//
// data_explorer.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//
use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::data_explorer_comm::ArraySelection;
use amalthea::comm::data_explorer_comm::ColumnDisplayType;
use amalthea::comm::data_explorer_comm::ColumnFilter;
use amalthea::comm::data_explorer_comm::ColumnFilterParams;
use amalthea::comm::data_explorer_comm::ColumnFilterType;
use amalthea::comm::data_explorer_comm::ColumnFrequencyTable;
use amalthea::comm::data_explorer_comm::ColumnFrequencyTableParams;
use amalthea::comm::data_explorer_comm::ColumnHistogram;
use amalthea::comm::data_explorer_comm::ColumnHistogramParams;
use amalthea::comm::data_explorer_comm::ColumnHistogramParamsMethod;
use amalthea::comm::data_explorer_comm::ColumnProfileParams;
use amalthea::comm::data_explorer_comm::ColumnProfileRequest;
use amalthea::comm::data_explorer_comm::ColumnProfileResult;
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
use amalthea::comm::data_explorer_comm::FilterMatchDataTypes;
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
use amalthea::comm::data_explorer_comm::SearchSchemaParams;
use amalthea::comm::data_explorer_comm::SearchSchemaResult;
use amalthea::comm::data_explorer_comm::SearchSchemaSortOrder;
use amalthea::comm::data_explorer_comm::Selection;
use amalthea::comm::data_explorer_comm::SetRowFiltersParams;
use amalthea::comm::data_explorer_comm::SetSortColumnsParams;
use amalthea::comm::data_explorer_comm::SummaryStatsBoolean;
use amalthea::comm::data_explorer_comm::SummaryStatsNumber;
use amalthea::comm::data_explorer_comm::SummaryStatsString;
use amalthea::comm::data_explorer_comm::TableSchema;
use amalthea::comm::data_explorer_comm::TableSelection;
use amalthea::comm::data_explorer_comm::TableSelectionKind;
use amalthea::comm::data_explorer_comm::TextSearchType;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket;
use amalthea::socket::comm::CommSocket;
use ark::data_explorer::format::format_column;
use ark::data_explorer::format::format_string;
use ark::data_explorer::r_data_explorer::DataObjectEnvInfo;
use ark::data_explorer::r_data_explorer::RDataExplorer;
use ark::fixtures::r_test_lock;
use ark::fixtures::socket_rpc_request;
use ark::lsp::events::EVENTS;
use ark::r_task::r_task;
use ark::thread::RThreadSafe;
use crossbeam::channel::bounded;
use harp::environment::R_ENVS;
use harp::object::RObject;
use harp::r_symbol;
use itertools::enumerate;
use itertools::Itertools;
use libr::R_GlobalEnv;
use libr::Rf_eval;
use stdext::assert_match;

// We don't care about events coming back quickly, we just don't want to deadlock
// in case something has gone wrong, so we pick a pretty long timeout to use throughout
// the tests.
static RECV_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

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
    let msg = comm_manager_rx.recv_timeout(RECV_TIMEOUT).unwrap();
    match msg {
        CommManagerEvent::Opened(socket, _value) => {
            assert_eq!(socket.comm_name, "positron.dataExplorer");
            socket
        },
        _ => panic!("Unexpected Comm Manager Event"),
    }
}

fn open_data_explorer_from_expression(
    expr: &str,
    bind: Option<&str>,
) -> anyhow::Result<socket::comm::CommSocket> {
    let (comm_manager_tx, comm_manager_rx) = bounded::<CommManagerEvent>(0);

    r_task(|| -> anyhow::Result<()> {
        let object = harp::parse_eval_global(expr)?;

        let binding = match bind {
            Some(name) => Some(DataObjectEnvInfo {
                name: name.to_string(),
                env: RThreadSafe::new(RObject::view(R_ENVS.global)),
            }),
            None => None,
        };
        RDataExplorer::start(String::from("obj"), object, binding, comm_manager_tx).unwrap();
        Ok(())
    })?;

    // Release the R lock and wait for the new comm to show up.
    let msg = comm_manager_rx.recv_timeout(RECV_TIMEOUT).unwrap();

    match msg {
        CommManagerEvent::Opened(socket, _value) => {
            assert_eq!(socket.comm_name, "positron.dataExplorer");
            Ok(socket)
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
    socket_rpc_request::<DataExplorerBackendRequest, DataExplorerBackendReply>(&socket, req)
}

/// Test setup helper that reduces boilerplate for common test initialization
struct TestSetup {
    socket: socket::comm::CommSocket,
}

impl TestSetup {
    fn new(dataset: &str) -> Self {
        Self {
            socket: open_data_explorer(String::from(dataset)),
        }
    }

    fn from_expression(expr: &str, bind: Option<&str>) -> anyhow::Result<Self> {
        Ok(Self {
            socket: open_data_explorer_from_expression(expr, bind)?,
        })
    }

    fn socket(&self) -> &socket::comm::CommSocket {
        &self.socket
    }
}

/// Request builder helpers to reduce repetitive request construction
struct RequestBuilder;

impl RequestBuilder {
    fn get_schema(column_indices: Vec<i64>) -> DataExplorerBackendRequest {
        DataExplorerBackendRequest::GetSchema(GetSchemaParams { column_indices })
    }

    fn get_data_values(
        row_start: i64,
        num_rows: i64,
        columns: Vec<i64>,
    ) -> DataExplorerBackendRequest {
        get_data_values_request(row_start, num_rows, columns, default_format_options())
    }

    fn search_schema_text(
        term: &str,
        search_type: TextSearchType,
        case_sensitive: bool,
        sort_order: SearchSchemaSortOrder,
    ) -> DataExplorerBackendRequest {
        DataExplorerBackendRequest::SearchSchema(SearchSchemaParams {
            filters: vec![ColumnFilter {
                filter_type: ColumnFilterType::TextSearch,
                params: ColumnFilterParams::TextSearch(FilterTextSearch {
                    search_type,
                    term: term.to_string(),
                    case_sensitive,
                }),
            }],
            sort_order,
        })
    }

    fn search_schema_data_types(
        display_types: Vec<ColumnDisplayType>,
        sort_order: SearchSchemaSortOrder,
    ) -> DataExplorerBackendRequest {
        DataExplorerBackendRequest::SearchSchema(SearchSchemaParams {
            filters: vec![ColumnFilter {
                filter_type: ColumnFilterType::MatchDataTypes,
                params: ColumnFilterParams::MatchDataTypes(FilterMatchDataTypes { display_types }),
            }],
            sort_order,
        })
    }

    fn search_schema_with_filters(
        filters: Vec<ColumnFilter>,
        sort_order: SearchSchemaSortOrder,
    ) -> DataExplorerBackendRequest {
        DataExplorerBackendRequest::SearchSchema(SearchSchemaParams {
            filters,
            sort_order,
        })
    }

    fn set_sort_columns(sort_keys: Vec<ColumnSortKey>) -> DataExplorerBackendRequest {
        DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams { sort_keys })
    }

    fn get_state() -> DataExplorerBackendRequest {
        DataExplorerBackendRequest::GetState
    }

    fn get_row_labels(selection: ArraySelection) -> DataExplorerBackendRequest {
        DataExplorerBackendRequest::GetRowLabels(GetRowLabelsParams {
            selection,
            format_options: default_format_options(),
        })
    }

    fn get_column_profiles(
        callback_id: String,
        profiles: Vec<ColumnProfileRequest>,
    ) -> DataExplorerBackendRequest {
        DataExplorerBackendRequest::GetColumnProfiles(GetColumnProfilesParams {
            callback_id,
            profiles,
            format_options: default_format_options(),
        })
    }

    fn set_row_filters(filters: Vec<RowFilter>) -> DataExplorerBackendRequest {
        DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams { filters })
    }

    fn export_data_selection(
        format: ExportFormat,
        selection: TableSelection,
    ) -> DataExplorerBackendRequest {
        DataExplorerBackendRequest::ExportDataSelection(ExportDataSelectionParams {
            format,
            selection,
        })
    }
}

/// Test assertion helpers with enhanced error reporting
struct TestAssertions;

impl TestAssertions {
    fn assert_schema_columns(
        socket: &socket::comm::CommSocket,
        column_indices: Vec<i64>,
        expected_count: usize,
    ) {
        let req = RequestBuilder::get_schema(column_indices.clone());
        assert_match!(socket_rpc(socket, req),
            DataExplorerBackendReply::GetSchemaReply(schema) => {
                assert_eq!(schema.columns.len(), expected_count,
                    "Schema column count mismatch for indices {:?}: expected {}, got {}",
                    column_indices, expected_count, schema.columns.len());
            }
        );
    }

    fn assert_search_matches(
        socket: &socket::comm::CommSocket,
        req: DataExplorerBackendRequest,
        expected_matches: Vec<i64>,
    ) {
        assert_match!(socket_rpc(socket, req),
            DataExplorerBackendReply::SearchSchemaReply(SearchSchemaResult { matches }) => {
                assert_eq!(matches, expected_matches,
                    "Search results mismatch: expected {:?}, got {:?}", expected_matches, matches);
            }
        );
    }

    fn assert_data_values_count(
        socket: &socket::comm::CommSocket,
        row_start: i64,
        num_rows: i64,
        columns: Vec<i64>,
        expected_column_count: usize,
        expected_row_count: usize,
    ) {
        let req = RequestBuilder::get_data_values(row_start, num_rows, columns.clone());
        assert_match!(socket_rpc(socket, req),
            DataExplorerBackendReply::GetDataValuesReply(data) => {
                assert_eq!(data.columns.len(), expected_column_count,
                    "Data values column count mismatch for request ({}, {}, {:?}): expected {}, got {}",
                    row_start, num_rows, columns, expected_column_count, data.columns.len());
                if expected_column_count > 0 {
                    assert_eq!(data.columns[0].len(), expected_row_count,
                        "Data values row count mismatch for request ({}, {}, {:?}): expected {}, got {}",
                        row_start, num_rows, columns, expected_row_count, data.columns[0].len());
                }
            }
        );
    }

    fn assert_sort_columns_applied(
        socket: &socket::comm::CommSocket,
        sort_keys: Vec<ColumnSortKey>,
    ) {
        let req = RequestBuilder::set_sort_columns(sort_keys.clone());
        assert_match!(socket_rpc(socket, req),
            DataExplorerBackendReply::SetSortColumnsReply() => {
                // Success - sort keys {:?} were applied
            }
        );
    }

    fn assert_state<F>(socket: &socket::comm::CommSocket, check: F)
    where
        F: FnOnce(&amalthea::comm::data_explorer_comm::BackendState),
    {
        let req = RequestBuilder::get_state();
        assert_match!(socket_rpc(socket, req),
            DataExplorerBackendReply::GetStateReply(state) => {
                check(&state);
            }
        );
    }

    fn assert_row_labels<F>(socket: &socket::comm::CommSocket, selection: ArraySelection, check: F)
    where
        F: FnOnce(&Vec<Vec<String>>),
    {
        let req = RequestBuilder::get_row_labels(selection);
        assert_match!(socket_rpc(socket, req),
            DataExplorerBackendReply::GetRowLabelsReply(row_labels) => {
                check(&row_labels.row_labels);
            }
        );
    }

    fn assert_data_values<F>(
        socket: &socket::comm::CommSocket,
        row_start: i64,
        num_rows: i64,
        columns: Vec<i64>,
        check: F,
    ) where
        F: FnOnce(&Vec<Vec<ColumnValue>>),
    {
        let req = get_data_values_request(row_start, num_rows, columns, default_format_options());
        assert_match!(socket_rpc(socket, req),
            DataExplorerBackendReply::GetDataValuesReply(data) => {
                check(&data.columns);
            }
        );
    }

    fn assert_row_filters_applied(
        socket: &socket::comm::CommSocket,
        filters: Vec<RowFilter>,
        expected_rows: i64,
        had_errors: Option<bool>,
    ) {
        let req = RequestBuilder::set_row_filters(filters.clone());
        assert_match!(socket_rpc(socket, req),
            DataExplorerBackendReply::SetRowFiltersReply(FilterResult {
                selected_num_rows,
                had_errors: actual_had_errors
            }) => {
                assert_eq!(selected_num_rows, expected_rows,
                    "Row filter result mismatch for filters {:?}: expected {} selected rows, got {}",
                    filters, expected_rows, selected_num_rows);
                assert_eq!(actual_had_errors, had_errors,
                    "Row filter error state mismatch for filters {:?}: expected had_errors {:?}, got {:?}",
                    filters, had_errors, actual_had_errors);
            }
        );
    }

    fn assert_export_data<F>(
        socket: &socket::comm::CommSocket,
        format: ExportFormat,
        selection: TableSelection,
        check: F,
    ) where
        F: FnOnce(&ExportedData),
    {
        let req = RequestBuilder::export_data_selection(format, selection);
        assert_match!(socket_rpc(socket, req),
            DataExplorerBackendReply::ExportDataSelectionReply(exported_data) => {
                check(&exported_data);
            }
        );
    }

    fn get_column_schema(
        socket: &socket::comm::CommSocket,
        column_indices: Vec<i64>,
    ) -> TableSchema {
        let req = RequestBuilder::get_schema(column_indices.clone());
        match socket_rpc(socket, req) {
            DataExplorerBackendReply::GetSchemaReply(schema) => schema,
            reply => panic!(
                "Expected GetSchemaReply for indices {:?}, got {:?}",
                column_indices, reply
            ),
        }
    }
}

/// Factories for creating common test data
struct TestDataBuilder;

impl TestDataBuilder {
    fn create_mixed_types_dataframe() -> anyhow::Result<TestSetup> {
        TestSetup::from_expression(
            "data.frame(
                name = c('Alice', 'Bob'),
                age = c(25L, 30L),
                score = c(95.5, 87.2),
                is_active = c(TRUE, FALSE),
                date_joined = as.Date(c('2021-01-01', '2021-02-01'))
            )",
            None,
        )
    }

    fn create_search_test_dataframe() -> anyhow::Result<TestSetup> {
        TestSetup::from_expression(
            "data.frame(
                user_name = c('Alice', 'Bob', 'Charlie'),
                user_age = c(25, 30, 35),
                user_id = c(1L, 2L, 3L),
                email_address = c('alice@test.com', 'bob@test.com', 'charlie@test.com'),
                admin_email = c('admin1@test.com', 'admin2@test.com', 'admin3@test.com'),
                score = c(95.5, 87.2, 92.1),
                bonus_score = c(10.0, 5.0, 8.5),
                is_active = c(TRUE, FALSE, TRUE),
                is_premium = c(FALSE, TRUE, FALSE),
                registration_date = as.Date(c('2021-01-01', '2021-02-01', '2021-03-01')),
                last_login = as.POSIXct(c('2024-01-01 10:00:00', '2024-01-02 11:00:00', '2024-01-03 12:00:00'))
            )",
            None,
        )
    }

    fn create_sort_order_test_dataframe() -> anyhow::Result<TestSetup> {
        TestSetup::from_expression(
            "data.frame(
                zebra = c(1, 2),
                apple = c(3, 4),
                banana = c(5, 6)
            )",
            None,
        )
    }
}

/// Helper to create common selections and data structures
struct SelectionBuilder;

impl SelectionBuilder {
    fn indices(indices: Vec<i64>) -> ArraySelection {
        ArraySelection::SelectIndices(DataSelectionIndices { indices })
    }

    fn single_cell(row_index: i64, column_index: i64) -> TableSelection {
        TableSelection {
            kind: TableSelectionKind::SingleCell,
            selection: Selection::SingleCell(DataSelectionSingleCell {
                row_index,
                column_index,
            }),
        }
    }

    fn column_sort_key(column_index: i64, ascending: bool) -> ColumnSortKey {
        ColumnSortKey {
            column_index,
            ascending,
        }
    }
}

/// Helper to create row filters easily
struct RowFilterBuilder;

impl RowFilterBuilder {
    fn comparison(
        column_schema: amalthea::comm::data_explorer_comm::ColumnSchema,
        op: FilterComparisonOp,
        value: &str,
    ) -> RowFilter {
        RowFilter {
            column_schema,
            filter_type: RowFilterType::Compare,
            params: Some(RowFilterParams::Comparison(FilterComparison {
                op,
                value: value.to_string(),
            })),
            filter_id: uuid::Uuid::new_v4().to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            error_message: None,
        }
    }

    fn text_search(
        column_schema: amalthea::comm::data_explorer_comm::ColumnSchema,
        search_type: TextSearchType,
        term: &str,
        case_sensitive: bool,
    ) -> RowFilter {
        RowFilter {
            column_schema,
            filter_type: RowFilterType::Search,
            params: Some(RowFilterParams::TextSearch(FilterTextSearch {
                search_type,
                term: term.to_string(),
                case_sensitive,
            })),
            filter_id: uuid::Uuid::new_v4().to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            error_message: None,
        }
    }

    fn is_null(column_schema: amalthea::comm::data_explorer_comm::ColumnSchema) -> RowFilter {
        RowFilter {
            column_schema,
            filter_type: RowFilterType::IsNull,
            params: None,
            filter_id: uuid::Uuid::new_v4().to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            error_message: None,
        }
    }

    fn not_null(column_schema: amalthea::comm::data_explorer_comm::ColumnSchema) -> RowFilter {
        RowFilter {
            column_schema,
            filter_type: RowFilterType::NotNull,
            params: None,
            filter_id: uuid::Uuid::new_v4().to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            error_message: None,
        }
    }

    fn is_empty(column_schema: amalthea::comm::data_explorer_comm::ColumnSchema) -> RowFilter {
        RowFilter {
            column_schema,
            filter_type: RowFilterType::IsEmpty,
            params: None,
            filter_id: uuid::Uuid::new_v4().to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            error_message: None,
        }
    }

    fn is_true(column_schema: amalthea::comm::data_explorer_comm::ColumnSchema) -> RowFilter {
        RowFilter {
            column_schema,
            filter_type: RowFilterType::IsTrue,
            params: None,
            filter_id: uuid::Uuid::new_v4().to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            error_message: None,
        }
    }

    fn is_false(column_schema: amalthea::comm::data_explorer_comm::ColumnSchema) -> RowFilter {
        RowFilter {
            column_schema,
            filter_type: RowFilterType::IsFalse,
            params: None,
            filter_id: uuid::Uuid::new_v4().to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            error_message: None,
        }
    }

    fn set_membership(
        column_schema: amalthea::comm::data_explorer_comm::ColumnSchema,
        values: Vec<String>,
        inclusive: bool,
    ) -> RowFilter {
        RowFilter {
            column_schema,
            filter_type: RowFilterType::SetMembership,
            params: Some(RowFilterParams::SetMembership(
                amalthea::comm::data_explorer_comm::FilterSetMembership { values, inclusive },
            )),
            filter_id: uuid::Uuid::new_v4().to_string(),
            condition: RowFilterCondition::And,
            is_valid: None,
            error_message: None,
        }
    }
}

/// Helper to create common column filters
struct FilterBuilder;

impl FilterBuilder {
    fn text_contains(term: &str, case_sensitive: bool) -> ColumnFilter {
        ColumnFilter {
            filter_type: ColumnFilterType::TextSearch,
            params: ColumnFilterParams::TextSearch(FilterTextSearch {
                search_type: TextSearchType::Contains,
                term: term.to_string(),
                case_sensitive,
            }),
        }
    }

    fn match_data_types(display_types: Vec<ColumnDisplayType>) -> ColumnFilter {
        ColumnFilter {
            filter_type: ColumnFilterType::MatchDataTypes,
            params: ColumnFilterParams::MatchDataTypes(FilterMatchDataTypes { display_types }),
        }
    }
}

/// Helper to create column profile requests
struct ProfileBuilder;

impl ProfileBuilder {
    fn null_count(column_index: i64) -> ColumnProfileRequest {
        ColumnProfileRequest {
            column_index,
            profiles: vec![ColumnProfileSpec {
                profile_type: ColumnProfileType::NullCount,
                params: None,
            }],
        }
    }

    fn summary_stats(column_index: i64) -> ColumnProfileRequest {
        ColumnProfileRequest {
            column_index,
            profiles: vec![ColumnProfileSpec {
                profile_type: ColumnProfileType::SummaryStats,
                params: None,
            }],
        }
    }

    fn small_histogram(
        column_index: i64,
        method: ColumnHistogramParamsMethod,
        num_bins: i64,
        quantiles: Option<Vec<f64>>,
    ) -> ColumnProfileRequest {
        ColumnProfileRequest {
            column_index,
            profiles: vec![ColumnProfileSpec {
                profile_type: ColumnProfileType::SmallHistogram,
                params: Some(ColumnProfileParams::SmallHistogram(ColumnHistogramParams {
                    method,
                    num_bins,
                    quantiles,
                })),
            }],
        }
    }

    fn small_frequency_table(column_index: i64, limit: i64) -> ColumnProfileRequest {
        ColumnProfileRequest {
            column_index,
            profiles: vec![ColumnProfileSpec {
                profile_type: ColumnProfileType::SmallFrequencyTable,
                params: Some(ColumnProfileParams::SmallFrequencyTable(
                    ColumnFrequencyTableParams { limit },
                )),
            }],
        }
    }
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

fn expect_column_profile_results(
    socket: &CommSocket,
    req: DataExplorerBackendRequest,
    check: fn(Vec<ColumnProfileResult>),
) {
    // Randomly generate a unique ID for this request.
    let id = uuid::Uuid::new_v4().to_string();

    // Serialize the message for the wire
    let json = serde_json::to_value(req).unwrap();
    println!("--> {:?}", json);

    // Convert the request to a CommMsg and send it.
    let msg = CommMsg::Rpc(id, json);
    socket.incoming_tx.send(msg).unwrap();

    let msg = socket.outgoing_rx.recv_timeout(RECV_TIMEOUT).unwrap();

    // Because during tests, no threads are created with r_task::spawn_idle, the messages are in
    // an incorrect order. We first receive the DataExplorerFrontndEvent with the column profiles
    // and then receive the results.
    assert_match!(
        msg,
        CommMsg::Data(value) => {
            let event = serde_json::from_value::<DataExplorerFrontendEvent>(value).unwrap();
            assert_match!(
                event,
                DataExplorerFrontendEvent::ReturnColumnProfiles(ev) => {
                    check(ev.profiles);
                }
            );
        }
    );

    let msg = socket.outgoing_rx.recv_timeout(RECV_TIMEOUT).unwrap();

    let reply: DataExplorerBackendReply = match msg {
        CommMsg::Rpc(_id, value) => {
            println!("<-- {:?}", value);
            let reply = serde_json::from_value(value).unwrap();
            reply
        },
        _ => panic!("Unexpected Comm Message"),
    };

    assert_eq!(reply, DataExplorerBackendReply::GetColumnProfilesReply());
}

fn test_mtcars_sort(socket: CommSocket, has_row_names: bool, display_name: String) {
    // Check that we got the right number of columns (mtcars has 11 columns)
    TestAssertions::assert_schema_columns(&socket, (0..11).collect(), 11);

    // Check that we can get data values (5 rows, 5 columns from middle of dataset)
    TestAssertions::assert_data_values_count(&socket, 5, 5, vec![0, 1, 2, 3, 4], 5, 5);

    // Check row names are present
    if has_row_names {
        TestAssertions::assert_row_labels(
            &socket,
            SelectionBuilder::indices(vec![5, 6, 7, 8, 9]),
            |labels| {
                assert_eq!(labels[0][0], "Valiant");
                assert_eq!(labels[0][1], "Duster 360");
                assert_eq!(labels[0][2], "Merc 240D");
            },
        );
    }

    // Sort by 'mpg' column (ascending)
    let mpg_sort_keys = vec![SelectionBuilder::column_sort_key(0, true)];
    TestAssertions::assert_sort_columns_applied(&socket, mpg_sort_keys.clone());

    // Verify the state shows correct sort keys and display name
    TestAssertions::assert_state(&socket, |state| {
        assert_eq!(state.display_name, display_name);
        assert_eq!(state.sort_keys, mpg_sort_keys);
    });

    // Check sorted values are correct (first three rows)
    TestAssertions::assert_data_values(&socket, 0, 3, vec![0, 1], |data| {
        assert_eq!(data.len(), 2);
        assert_eq!(data[0].len(), 3);
        assert_eq!(data[0][0], ColumnValue::FormattedValue("10.40".to_string()));
        assert_eq!(data[0][1], ColumnValue::FormattedValue("10.40".to_string()));
        assert_eq!(data[0][2], ColumnValue::FormattedValue("13.30".to_string()));
    });

    // Row labels should be sorted as well
    if has_row_names {
        TestAssertions::assert_row_labels(
            &socket,
            SelectionBuilder::indices(vec![0, 1, 2]),
            |labels| {
                assert_eq!(labels[0][0], "Cadillac Fleetwood");
                assert_eq!(labels[0][1], "Lincoln Continental");
                assert_eq!(labels[0][2], "Camaro Z28");
            },
        );
    }

    // More complex sort: by 'cyl' descending, then by 'mpg' descending
    let descending_sort_keys = vec![
        SelectionBuilder::column_sort_key(1, false),
        SelectionBuilder::column_sort_key(0, false),
    ];
    TestAssertions::assert_sort_columns_applied(&socket, descending_sort_keys);

    // Check the complex sorted values
    TestAssertions::assert_data_values(&socket, 0, 3, vec![0, 1], |data| {
        assert_eq!(data.len(), 2);
        assert_eq!(data[0][0], ColumnValue::FormattedValue("19.20".to_string()));
        assert_eq!(data[0][1], ColumnValue::FormattedValue("18.70".to_string()));
        assert_eq!(data[0][2], ColumnValue::FormattedValue("17.30".to_string()));
    });
}

#[test]
fn test_basic_mtcars() {
    let _lock = r_test_lock();
    let setup = TestSetup::new("mtcars");
    test_mtcars_sort(setup.socket, true, String::from("mtcars"));
}

#[test]
fn test_tibble_support() {
    let _lock = r_test_lock();

    let has_tibble =
        r_task(|| harp::parse_eval_global("mtcars_tib <- tibble::as_tibble(mtcars)").is_ok());
    if !has_tibble {
        return;
    }

    let setup = TestSetup::new("mtcars_tib");
    test_mtcars_sort(setup.socket, false, String::from("mtcars_tib"));

    r_task(|| {
        harp::parse_eval_global("rm(mtcars_tib)").unwrap();
    });
}

#[test]
fn test_women_dataset() {
    let _lock = r_test_lock();
    let setup = TestSetup::new("women");
    let socket = setup.socket();

    // Check initial data values (first 2 rows)
    TestAssertions::assert_data_values(socket, 0, 2, vec![0, 1], |data| {
        assert_eq!(data.len(), 2);
        assert_eq!(data[0][0], ColumnValue::FormattedValue("58.00".to_string()));
        assert_eq!(data[0][1], ColumnValue::FormattedValue("59.00".to_string()));
    });

    // Check row names
    TestAssertions::assert_row_labels(socket, SelectionBuilder::indices(vec![0, 1, 2]), |labels| {
        assert_eq!(labels[0][0], "1");
        assert_eq!(labels[0][1], "2");
        assert_eq!(labels[0][2], "3");
    });

    // Sort by height (descending)
    let sort_keys = vec![SelectionBuilder::column_sort_key(0, false)];
    TestAssertions::assert_sort_columns_applied(socket, sort_keys);

    // Get schema to use for filtering
    let req = RequestBuilder::get_schema(vec![0, 1]);
    let schema = match socket_rpc(socket, req) {
        DataExplorerBackendReply::GetSchemaReply(schema) => schema,
        _ => panic!("Expected schema reply"),
    };

    // Apply filter: height < 60
    let filters = vec![RowFilterBuilder::comparison(
        schema.columns[0].clone(),
        FilterComparisonOp::Lt,
        "60",
    )];
    TestAssertions::assert_row_filters_applied(socket, filters, 2, Some(false));

    // Check filtered and sorted data
    TestAssertions::assert_data_values(socket, 0, 2, vec![0, 1], |data| {
        assert_eq!(data.len(), 2);
        assert_eq!(data[0][0], ColumnValue::FormattedValue("59.00".to_string()));
        assert_eq!(data[0][1], ColumnValue::FormattedValue("58.00".to_string()));
    });
}

#[test]
fn test_matrix_support() {
    let _lock = r_test_lock();
    let setup = TestSetup::new("volcano");
    let socket = setup.socket();

    // Verify volcano matrix has 61 columns
    TestAssertions::assert_schema_columns(socket, (0..61).collect_vec(), 61);

    // Get schema for filtering
    let req = RequestBuilder::get_schema((0..61).collect_vec());
    let schema = match socket_rpc(socket, req) {
        DataExplorerBackendReply::GetSchemaReply(schema) => schema,
        _ => panic!("Expected schema reply"),
    };

    // Sort by first column (ascending)
    let sort_keys = vec![SelectionBuilder::column_sort_key(0, true)];
    TestAssertions::assert_sort_columns_applied(socket, sort_keys);

    // Check sorted data values (first 4 rows, 2 columns)
    TestAssertions::assert_data_values(socket, 0, 4, vec![0, 1], |data| {
        assert_eq!(data.len(), 2);
        assert_eq!(data[0][0], ColumnValue::FormattedValue("97.00".to_string()));
        assert_eq!(data[0][1], ColumnValue::FormattedValue("97.00".to_string()));
        assert_eq!(data[0][2], ColumnValue::FormattedValue("98.00".to_string()));
        assert_eq!(data[0][3], ColumnValue::FormattedValue("98.00".to_string()));
    });

    // Apply filter: first column < 100
    let filters = vec![RowFilterBuilder::comparison(
        schema.columns[0].clone(),
        FilterComparisonOp::Lt,
        "100",
    )];
    TestAssertions::assert_row_filters_applied(socket, filters, 8, Some(false));
}

#[test]
fn test_data_table_support() {
    let _lock = r_test_lock();

    let has_data_table =
        r_task(|| harp::parse_eval_global("mtcars_dt <- data.table::data.table(mtcars)").is_ok());
    if !has_data_table {
        return;
    }

    let setup = TestSetup::new("mtcars_dt");
    test_mtcars_sort(setup.socket, false, String::from("mtcars_dt"));

    r_task(|| {
        harp::parse_eval_global("rm(mtcars_dt)").unwrap();
    });
}

#[test]
fn test_null_counts() {
    let _lock = r_test_lock();
    let setup = TestSetup::from_expression(
        "fibo <- data.frame(col = c(1, NA, 2, 3, 5, NA, 13, 21, NA))",
        None,
    )
    .unwrap();
    let socket = setup.socket();

    // Get schema for filtering
    let req = RequestBuilder::get_schema(vec![0]);
    let schema = match socket_rpc(socket, req) {
        DataExplorerBackendReply::GetSchemaReply(schema) => schema,
        _ => panic!("Expected schema reply"),
    };

    // Check null count (should be 3)
    let req =
        RequestBuilder::get_column_profiles(String::from("id"), vec![ProfileBuilder::null_count(
            0,
        )]);
    expect_column_profile_results(socket, req, |data| {
        assert_eq!(data.len(), 1);
        assert_eq!(data[0].null_count, Some(3));
    });

    // Filter out null values (NotNull filter)
    let filters = vec![RowFilterBuilder::not_null(schema.columns[0].clone())];
    TestAssertions::assert_row_filters_applied(socket, filters, 6, Some(false));

    // Null count should now be 0 (after filtering out nulls)
    let req =
        RequestBuilder::get_column_profiles(String::from("id2"), vec![ProfileBuilder::null_count(
            0,
        )]);
    expect_column_profile_results(socket, req, |data| {
        assert_eq!(data.len(), 1);
        assert_eq!(data[0].null_count, Some(0));
    });

    // Filter to show ONLY null values (IsNull filter)
    let filters = vec![RowFilterBuilder::is_null(schema.columns[0].clone())];
    TestAssertions::assert_row_filters_applied(socket, filters, 3, Some(false));
}

#[test]
fn test_summary_stats() {
    let _lock = r_test_lock();

    // Create test data with mixed types for summary statistics
    r_task(|| {
        harp::parse_eval_global(
            "df <- data.frame(num = c(1, 2, 3, NA), char = c('a', 'a', '', NA), bool = c(TRUE, TRUE, FALSE, NA))"
        ).unwrap();
    });

    let setup = TestSetup::new("df");
    let socket = setup.socket();

    // Request summary stats for all 3 columns
    let req = RequestBuilder::get_column_profiles(
        String::from("id"),
        (0..3).map(|i| ProfileBuilder::summary_stats(i)).collect(),
    );

    expect_column_profile_results(socket, req, |data| {
        assert_eq!(data.len(), 3);

        // First column: numeric stats
        assert!(data[0].summary_stats.is_some());
        let number_stats = data[0].summary_stats.clone().unwrap().number_stats.unwrap();
        assert_eq!(number_stats, SummaryStatsNumber {
            min_value: Some(String::from("1.00")),
            max_value: Some(String::from("3.00")),
            mean: Some(String::from("2.00")),
            median: Some(String::from("2.00")),
            stdev: Some(String::from("1.00")),
        });

        // Second column: character stats
        assert!(data[1].summary_stats.is_some());
        let string_stats = data[1].summary_stats.clone().unwrap().string_stats.unwrap();
        assert_eq!(string_stats, SummaryStatsString {
            num_empty: 1,
            num_unique: 3, // NA's are counted as unique values
        });

        // Third column: boolean stats
        assert!(data[2].summary_stats.is_some());
        let boolean_stats = data[2]
            .summary_stats
            .clone()
            .unwrap()
            .boolean_stats
            .unwrap();
        assert_eq!(boolean_stats, SummaryStatsBoolean {
            true_count: 2,
            false_count: 1,
        });
    });
}

#[test]
fn test_search_filters() {
    let _lock = r_test_lock();

    // Create test data with various text patterns
    r_task(|| {
        harp::parse_eval_global(
            r#"words <- data.frame(text = c(
                    "lambent",
                    "incandescent",
                    "that will be $10.26",
                    "pi is 3.14159",
                    "",
                    "weasel",
                    "refrigerator"
                ))"#,
        )
        .unwrap();
    });

    let setup = TestSetup::new("words");
    let socket = setup.socket();

    // Get schema for filtering
    let req = RequestBuilder::get_schema(vec![0]);
    let schema = match socket_rpc(socket, req) {
        DataExplorerBackendReply::GetSchemaReply(schema) => schema,
        _ => panic!("Expected schema reply"),
    };

    // Filter for text containing "." (matches 2 rows)
    let dot_filter = RowFilterBuilder::text_search(
        schema.columns[0].clone(),
        TextSearchType::Contains,
        ".",
        false,
    );
    TestAssertions::assert_row_filters_applied(socket, vec![dot_filter.clone()], 2, Some(false));

    // Combine filters: contains "." OR ends with "ent" (matches 4 rows)
    let mut ent_filter = RowFilterBuilder::text_search(
        schema.columns[0].clone(),
        TextSearchType::EndsWith,
        "ent",
        false,
    );
    ent_filter.condition = RowFilterCondition::Or;
    TestAssertions::assert_row_filters_applied(
        socket,
        vec![dot_filter, ent_filter],
        4,
        Some(false),
    );

    // Filter for empty values (matches 1 row)
    let empty_filter = RowFilterBuilder::is_empty(schema.columns[0].clone());
    TestAssertions::assert_row_filters_applied(socket, vec![empty_filter], 1, Some(false));

    // Check table state: 1 row visible out of 7 total
    TestAssertions::assert_state(socket, |state| {
        assert_eq!(state.table_shape.num_rows, 1);
        assert_eq!(state.table_unfiltered_shape.num_rows, 7);
    });

    // --- invalid filters ---

    // Create a data frame with a bunch of dates.
    r_task(|| {
        harp::parse_eval_global(
            r#"test_dates <- data.frame(date = as.POSIXct(c(
                    "2024-01-01 01:00:00",
                    "2024-01-02 02:00:00",
                    "2024-01-03 03:00:00"))
            )"#,
        )
        .unwrap();
    });

    // Open the dates data set in the data explorer.
    let socket = open_data_explorer(String::from("test_dates"));

    // Get the schema of the data set.
    let schema = TestAssertions::get_column_schema(&socket, vec![0]);

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
    let req = RequestBuilder::set_row_filters(vec![year_filter.clone()]);

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
    let bools_setup = TestSetup::from_expression(
        r#"test_bools <- data.frame(bool = c(
                    TRUE,
                    TRUE,
                    FALSE,
                    NA,
                    TRUE,
                    FALSE
            ))"#,
        Some("test_bools"),
    )
    .unwrap();
    let bools_socket = bools_setup.socket();
    let bools_schema = TestAssertions::get_column_schema(&bools_socket, vec![0]);

    // Apply boolean filter: check for TRUE values
    let true_filter = RowFilterBuilder::is_true(bools_schema.columns[0].clone());
    TestAssertions::assert_row_filters_applied(bools_socket, vec![true_filter], 3, Some(false));

    // Apply boolean filter: check for FALSE values
    let false_filter = RowFilterBuilder::is_false(bools_schema.columns[0].clone());
    TestAssertions::assert_row_filters_applied(bools_socket, vec![false_filter], 2, Some(false));

    let has_tibble = r_task(|| {
        harp::parse_eval_global(
            r#"list_cols <- tibble::tibble(
                list_col = list(c(1,2,3,4), tibble::tibble(x = 1, b = 2), matrix(1:4, nrow = 2), c(TRUE, FALSE)),
                list_col_class = vctrs::list_of(1,2,3, 4)
            )"#,
        ).is_ok()
    });
    if !has_tibble {
        return;
    }

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
}

#[test]
fn test_live_updates() {
    let _lock = r_test_lock();

    let socket = open_data_explorer_from_expression(
        "x <- data.frame(y = c(3, 2, 1), z = c(4, 5, 6))",
        Some("x"),
    )
    .unwrap();

    // Make a data-level change to the data set.
    r_task(|| {
        harp::parse_eval_global("x[1, 1] <- 0").unwrap();
    });

    // Emit a console prompt event; this should tickle the data explorer to
    // check for changes.
    EVENTS.console_prompt.emit(());

    // Wait for an update event to arrive
    assert_match!(socket.outgoing_rx.recv_timeout(RECV_TIMEOUT).unwrap(),
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
    r_task(|| {
        harp::parse_eval_global("x[1, 1] <- 3").unwrap();
    });

    // Emit a console prompt event; this should tickle the data explorer to
    // check for changes.
    EVENTS.console_prompt.emit(());

    // Wait for an update event to arrive
    assert_match!(socket.outgoing_rx.recv_timeout(RECV_TIMEOUT).unwrap(),
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
    r_task(|| {
        harp::parse_eval_global("x <- data.frame(y = 'y', z = 'z', three = '3')").unwrap();
    });

    // Emit a console prompt event to trigger change detection
    EVENTS.console_prompt.emit(());

    // This should trigger a schema update event.
    assert_match!(socket.outgoing_rx.recv_timeout(RECV_TIMEOUT).unwrap(),
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
    r_task(|| {
        harp::parse_eval_global("rm(x)").unwrap();
    });

    // Emit a console prompt event to trigger change detection
    EVENTS.console_prompt.emit(());

    // Wait for an close event to arrive
    assert_match!(socket.outgoing_rx.recv_timeout(RECV_TIMEOUT).unwrap(),
        CommMsg::Close => {}
    );
}

// Tests that invalid filters are preserved after a live update that removes the column
// Refer to https://github.com/posit-dev/positron/issues/3141 for more info.
#[test]
fn test_invalid_filters_preserved() {
    let _lock = r_test_lock();
    let socket = open_data_explorer_from_expression(
        r#"test_df <- data.frame(x = c('','a', 'b'), y = c(1, 2, 3))"#,
        Some("test_df"),
    )
    .unwrap();

    // Get the schema of the data set.
    let schema = TestAssertions::get_column_schema(&socket, vec![0]);

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

    let req = RequestBuilder::set_row_filters(vec![x_is_empty.clone()]);

    // We should get a SetRowFiltersReply back and we should get a single row
    assert_match!(socket_rpc(&socket, req),
    DataExplorerBackendReply::SetRowFiltersReply(
        FilterResult { selected_num_rows: num_rows, had_errors: Some(false)}
    ) => {
        assert_eq!(num_rows, 1);
    });

    // Now let's update the data frame removing the 'x' column, the filter should
    // now be invalid.
    r_task(|| {
        harp::parse_eval_global("test_df$x <- NULL").unwrap();
    });

    // Emit a console prompt event; this should tickle the data explorer to
    // check for changes.
    EVENTS.console_prompt.emit(());

    // Wait for an update event to arrive
    assert_match!(socket.outgoing_rx.recv_timeout(RECV_TIMEOUT).unwrap(),
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

    // We now re-assign the column to make the filter valid again and see if it's re-applied
    r_task(|| {
        harp::parse_eval_global("test_df$x <- c('','a', 'b')").unwrap();
    });

    // Emit a console prompt event; this should tickle the data explorer to
    // check for changes.
    EVENTS.console_prompt.emit(());

    // Wait for an update event to arrive
    assert_match!(socket.outgoing_rx.recv_timeout(RECV_TIMEOUT).unwrap(),
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

    // Now make the filter invalid because of the data type has changed
    r_task(|| {
        harp::parse_eval_global("test_df$x <- c(1, 2, 3)").unwrap();
    });

    // Emit a console prompt event; this should tickle the data explorer to
    // check for changes.
    EVENTS.console_prompt.emit(());

    // Wait for an update event to arrive
    assert_match!(socket.outgoing_rx.recv_timeout(RECV_TIMEOUT).unwrap(),
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

    r_task(|| {
        harp::parse_eval_global("rm(test_df)").unwrap();
    });
}

#[test]
fn test_data_explorer_special_values() {
    let _lock = r_test_lock();

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

    r_task(|| {
        harp::parse_eval_global("rm(x)").unwrap();
    });
}

// The main exporting logic is tested in the data_exporter module. This test
// is mainly an integration test to check if the data explorer can correctly
// work with sorting/filtering the data and then exporting it.
#[test]
fn test_export_data() {
    let _lock = r_test_lock();
    let socket = open_data_explorer_from_expression(
        r#"data.frame(
                a = c(1, 3, 2),
                b = c('a', 'b', 'c'),
                c = c(TRUE, FALSE, TRUE)
            )"#,
        None,
    )
    .unwrap();

    let single_cell_selection = SelectionBuilder::single_cell(1, 1);

    // Test initial export: should get "b" (row 1, col 1)
    TestAssertions::assert_export_data(
        &socket,
        ExportFormat::Csv,
        single_cell_selection.clone(),
        |exported| {
            assert_eq!(exported.data, "b".to_string());
            assert_eq!(exported.format, ExportFormat::Csv);
        },
    );

    // Sort descending by column 0, then test export: should get "c"
    let req = RequestBuilder::set_sort_columns(vec![ColumnSortKey {
        column_index: 0,
        ascending: false,
    }]);
    socket_rpc(&socket, req);

    TestAssertions::assert_export_data(
        &socket,
        ExportFormat::Csv,
        single_cell_selection.clone(),
        |exported| {
            assert_eq!(exported.data, "c".to_string());
        },
    );

    // Filter to show only TRUE values in boolean column, then test export: should get "a"
    let schema = TestAssertions::get_column_schema(&socket, vec![0, 1, 2]);
    let filter = RowFilterBuilder::is_true(schema.columns[2].clone());
    let req = RequestBuilder::set_row_filters(vec![filter]);
    socket_rpc(&socket, req);

    TestAssertions::assert_export_data(
        &socket,
        ExportFormat::Csv,
        single_cell_selection,
        |exported| {
            assert_eq!(exported.data, "a".to_string());
        },
    );
}

// Tests that filters and sorts are reapplied to new data after a Data Update event.
// A regression test for https://github.com/posit-dev/positron/issues/4170
#[test]
fn test_update_data_filters_reapplied() {
    let _lock = r_test_lock();

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
    let schema = TestAssertions::get_column_schema(&socket, vec![0]);

    // Apply filter by the `a` columns. Expecting to get 3 rows larger than 1.
    let x_gt_1 =
        RowFilterBuilder::comparison(schema.columns[0].clone(), FilterComparisonOp::Gt, "1");
    TestAssertions::assert_row_filters_applied(&socket, vec![x_gt_1.clone()], 3, Some(false));

    // Also add a sorting to check that data will be sorted in the correct way
    // after the data update.
    // Sort by column 0 ascending
    let req = RequestBuilder::set_sort_columns(vec![ColumnSortKey {
        column_index: 0,
        ascending: true,
    }]);
    socket_rpc(&socket, req);

    // Check the number of rows when using the GetData method
    let expect_get_data_rows = |n, values| {
        TestAssertions::assert_data_values(&socket, 0, 5, vec![0, 1], |data| {
            assert_eq!(data[0].len(), n);
            assert_eq!(data[1], values);
        });
    };

    // GetData should also display 2 rows only
    expect_get_data_rows(3, vec![
        ColumnValue::FormattedValue("a".to_string()),
        ColumnValue::FormattedValue("b".to_string()),
        ColumnValue::FormattedValue("c".to_string()),
    ]);

    // Now make the filter invalid because of the data type has changed
    r_task(|| {
        harp::parse_eval_global("x$a <- c(3, 2, 1, 1)").unwrap();
    });

    // Emit a console prompt event; this should tickle the data explorer to
    // check for changes.
    EVENTS.console_prompt.emit(());

    // Wait for an update event to arrive
    // Since only data changed, we expect a Data Update Event
    assert_match!(socket.outgoing_rx.recv_timeout(RECV_TIMEOUT).unwrap(),
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
}

/// Helper function to test set membership filters for both inclusive and exclusive modes
fn test_set_membership_helper(
    data_frame_name: &str,
    filter_values: Vec<&str>,
    expected_inclusive_count: usize,
    expected_exclusive_count: usize,
) {
    let socket = open_data_explorer(String::from(data_frame_name));

    let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
        column_indices: vec![0],
    });

    let schema_reply = socket_rpc(&socket, req);
    let schema = match schema_reply {
        DataExplorerBackendReply::GetSchemaReply(schema) => schema,
        _ => panic!("Unexpected reply: {:?}", schema_reply),
    };

    let string_values: Vec<String> = filter_values.iter().map(|s| s.to_string()).collect();

    let inclusive_filter = RowFilterBuilder::set_membership(
        schema.columns[0].clone(),
        string_values.clone(),
        true, // inclusive
    );

    let req = RequestBuilder::set_row_filters(vec![inclusive_filter]);

    assert_match!(socket_rpc(&socket, req),
    DataExplorerBackendReply::SetRowFiltersReply(
        FilterResult { selected_num_rows: num_rows, had_errors: Some(false) }
    ) => {
        assert_eq!(num_rows as usize, expected_inclusive_count,
                 "Inclusive filter for {} with values {:?} returned {} rows instead of expected {}",
                 data_frame_name, filter_values, num_rows, expected_inclusive_count);
    });

    let exclusive_filter = RowFilterBuilder::set_membership(
        schema.columns[0].clone(),
        string_values,
        false, // exclusive
    );

    let req = RequestBuilder::set_row_filters(vec![exclusive_filter]);

    assert_match!(socket_rpc(&socket, req),
    DataExplorerBackendReply::SetRowFiltersReply(
        FilterResult { selected_num_rows: num_rows, had_errors: Some(false) }
    ) => {
        assert_eq!(num_rows as usize, expected_exclusive_count,
                 "Exclusive filter for {} with values {:?} returned {} rows instead of expected {}",
                 data_frame_name, filter_values, num_rows, expected_exclusive_count);
    });
}

#[test]
fn test_set_membership_filter() {
    let _lock = r_test_lock();

    r_task(|| {
        harp::parse_eval_global(
            r#"categories <- data.frame(
                fruit = c(
                    "apple",
                    "banana",
                    "orange",
                    "grape",
                    "kiwi",
                    "pear",
                    "strawberry"
                )
            )"#,
        )
        .unwrap();
    });

    test_set_membership_helper(
        "categories",                    // data frame name
        vec!["apple", "banana", "pear"], // filter values
        3,                               // expected inclusive match count
        4,                               // expected exclusive match count
    );

    r_task(|| {
        harp::parse_eval_global(
            r#"numeric_data <- data.frame(
                values = c(1, 2, 3, 4, 5, 6, 7)
            )"#,
        )
        .unwrap();
    });

    test_set_membership_helper(
        "numeric_data",      // data frame name
        vec!["1", "2", "3"], // filter values (as strings, will be coerced)
        3,                   // expected inclusive match count
        4,                   // expected exclusive match count
    );

    // Test string data frame with NA values
    r_task(|| {
        harp::parse_eval_global(
            r#"categories_with_na <- data.frame(
                fruits = c(
                    "apple",
                    "banana",
                    NA_character_,
                    "orange",
                    "grape",
                    NA_character_,
                    "pear"
                )
            )"#,
        )
        .unwrap();
    });

    // Test with just regular values in the filter (NA values won't match)
    test_set_membership_helper("categories_with_na", vec!["apple", "banana"], 2, 5);

    // Test numeric data frame with NA values
    r_task(|| {
        harp::parse_eval_global(
            r#"numeric_with_na <- data.frame(
                values = c(1, 2, NA_real_, 3, NA_real_, 4, 5)
            )"#,
        )
        .unwrap();
    });

    // Tests with just regular values in the filter (NA values won't match)
    test_set_membership_helper("numeric_with_na", vec!["1", "2"], 2, 5);
    test_set_membership_helper("numeric_with_na", vec![], 0, 7);
    test_set_membership_helper("numeric_with_na", vec!["3"], 1, 6);
}

#[test]
fn test_get_data_values_by_indices() {
    let _lock = r_test_lock();

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
}

#[test]
fn test_data_update_num_rows() {
    let _lock = r_test_lock();

    // Regression test for https://github.com/posit-dev/positron/issues/4286
    // We test that after sending the data update event we also correctly update the
    // new number of rows.
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

    let req = DataExplorerBackendRequest::GetState;
    assert_match!(socket_rpc(&socket, req), DataExplorerBackendReply::GetStateReply(backend_state) => {
        assert_eq!(backend_state.table_shape.num_rows, 4);
    });

    // Now change the number of rows. The schema didn't change, so we should
    // recieve a data update event.
    r_task(|| {
        harp::parse_eval_global("x <- utils::tail(x, 2)").unwrap();
    });

    // Emit a console prompt event; this should tickle the data explorer to
    // check for changes.
    EVENTS.console_prompt.emit(());

    // Wait for an update event to arrive
    assert_match!(socket.outgoing_rx.recv_timeout(RECV_TIMEOUT).unwrap(),
        CommMsg::Data(value) => {
            // Make sure it's a data update event.
            assert_match!(serde_json::from_value::<DataExplorerFrontendEvent>(value).unwrap(),
                DataExplorerFrontendEvent::DataUpdate
            );
    });

    // Now get the shape and check num rows.
    let req = DataExplorerBackendRequest::GetState;
    assert_match!(socket_rpc(&socket, req), DataExplorerBackendReply::GetStateReply(backend_state) => {
        assert_eq!(backend_state.table_shape.num_rows, 2);
    });
}

#[test]
fn test_histogram() {
    let _lock = r_test_lock();

    let socket =
        open_data_explorer_from_expression("data.frame(x = rep(1:10, 10:1))", None).unwrap();

    let histogram_req =
        ProfileBuilder::small_histogram(0, ColumnHistogramParamsMethod::Fixed, 10, None);
    let req = RequestBuilder::get_column_profiles("histogram_req".to_string(), vec![histogram_req]);

    expect_column_profile_results(&socket, req, |profiles| {
        let histogram = profiles[0].small_histogram.clone().unwrap();
        assert_eq!(histogram, ColumnHistogram {
            bin_edges: r_task(|| format_string(
                harp::parse_eval_global("seq(1, 10, length.out=11)")
                    .unwrap()
                    .sexp,
                &default_format_options()
            )),
            bin_counts: vec![10, 9, 8, 7, 6, 5, 4, 3, 2, 1], // Pretty bind edges unite the first two intervals
            quantiles: vec![],
        });
    });
}

#[test]
fn test_histogram_single_bin_same_values() {
    let _lock = r_test_lock();

    let socket = open_data_explorer_from_expression("data.frame(x = rep(5, 10))", None).unwrap();

    let histogram_req =
        ProfileBuilder::small_histogram(0, ColumnHistogramParamsMethod::Fixed, 5, None);
    let req = RequestBuilder::get_column_profiles("histogram_same_values".to_string(), vec![
        histogram_req,
    ]);

    expect_column_profile_results(&socket, req, |profiles| {
        let histogram = profiles[0].small_histogram.clone().unwrap();

        // When all values are the same, we should get a single bin with count = number of values
        assert_eq!(histogram.bin_counts, vec![10]);

        // The bin edges should be [5, 5] since all values are 5
        let expected_edges = r_task(|| {
            format_string(
                harp::parse_eval_global("c(5, 5)").unwrap().sexp,
                &default_format_options(),
            )
        });
        assert_eq!(histogram.bin_edges, expected_edges);
        assert_eq!(histogram.quantiles, vec![]);
    });
}

#[test]
fn test_frequency_table() {
    let _lock = r_test_lock();

    let socket =
        open_data_explorer_from_expression("data.frame(x = rep(letters[1:10], 10:1))", None)
            .unwrap();

    let freq_table_req = ProfileBuilder::small_frequency_table(0, 5);
    let req = RequestBuilder::get_column_profiles("freq_table".to_string(), vec![freq_table_req]);

    expect_column_profile_results(&socket, req, |profiles| {
        let freq_table = profiles[0].small_frequency_table.clone().unwrap();
        assert_eq!(freq_table, ColumnFrequencyTable {
            values: format_column(
                harp::parse_eval_global("letters[1:5]").unwrap().sexp,
                &default_format_options()
            ),
            counts: vec![10, 9, 8, 7, 6],
            other_count: Some(5 + 4 + 3 + 2 + 1)
        });
    });
}

#[test]
fn test_row_names_matrix() {
    let _lock = r_test_lock();

    // Convert mtcars to a matrix
    let socket =
        open_data_explorer_from_expression("as.matrix(mtcars)", Some("mtcars_matrix")).unwrap();

    // Check row names are present
    let req = DataExplorerBackendRequest::GetRowLabels(GetRowLabelsParams {
        selection: ArraySelection::SelectIndices(DataSelectionIndices {
            indices: vec![5, 6, 7, 8, 9],
        }),
        format_options: default_format_options(),
    });
    assert_match!(socket_rpc(&socket, DataExplorerBackendRequest::GetState),
        DataExplorerBackendReply::GetStateReply(state) => {
            assert_eq!(state.has_row_labels, true)
        }
    );

    assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::GetRowLabelsReply(row_labels) => {
            let labels = row_labels.row_labels;
            assert_eq!(labels[0][0], "Valiant");
            assert_eq!(labels[0][1], "Duster 360");
            assert_eq!(labels[0][2], "Merc 240D");
        }
    );

    // Convert mtcars to a matrix
    let socket =
        open_data_explorer_from_expression("matrix(0, ncol =10, nrow = 10)", Some("zero_matrix"))
            .unwrap();
    assert_match!(socket_rpc(&socket, DataExplorerBackendRequest::GetState),
        DataExplorerBackendReply::GetStateReply(state) => {
            assert_eq!(state.has_row_labels, false)
        }
    );
}

#[test]
fn test_schema_identification() {
    let _lock = r_test_lock();
    let socket = open_data_explorer_from_expression(
        "data.frame(
            a = c(1, 2, 3),
            b = c('a', 'b', 'c'),
            c = c(TRUE, FALSE, TRUE),
            d = factor(c('a', 'b', 'c')),
            e = as.Date(c('2021-01-01', '2021-01-02', '2021-01-03')),
            f = as.POSIXct(c('2021-01-01 01:00:00', '2021-01-02 02:00:00', '2021-01-03 03:00:00'))
        )",
        None,
    )
    .unwrap();

    let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
        column_indices: vec![0, 1, 2, 3, 4, 5],
    });

    assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::GetSchemaReply(schema) => {
            assert_eq!(schema.columns.len(), 6);

            let expected_types = vec![
                (ColumnDisplayType::Number, "dbl"),
                (ColumnDisplayType::String, "str"),
                (ColumnDisplayType::Boolean, "lgl"),
                (ColumnDisplayType::String, "fct(3)"),
                (ColumnDisplayType::Date, "Date"),
                (ColumnDisplayType::Datetime, "POSIXct"),
            ];

            for (i, (expected_display, expected_name)) in expected_types.iter().enumerate() {
                assert_eq!(schema.columns[i].type_display, *expected_display);
                assert_eq!(schema.columns[i].type_name, expected_name.to_string());
            }
        }
    );
}

#[test]
fn test_search_schema_text_filters() {
    let _lock = r_test_lock();
    let setup = TestDataBuilder::create_search_test_dataframe().unwrap();
    let socket = setup.socket();

    // Schema: user_name(0), user_age(1), user_id(2), email_address(3), admin_email(4),
    //         score(5), bonus_score(6), is_active(7), is_premium(8), registration_date(9), last_login(10)

    // Test contains search: 'user' should match user_name, user_age, user_id
    let req = RequestBuilder::search_schema_text(
        "user",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![0, 1, 2]);

    // Test starts_with search: 'email' should match email_address only (not admin_email)
    let req = RequestBuilder::search_schema_text(
        "email",
        TextSearchType::StartsWith,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![3]);

    // Test ends_with search: 'active' should match is_active only (not is_premium)
    let req = RequestBuilder::search_schema_text(
        "active",
        TextSearchType::EndsWith,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![7]);

    // Test ends_with search: 'email' should match admin_email only (email_address ends with 'address')
    let req = RequestBuilder::search_schema_text(
        "email",
        TextSearchType::EndsWith,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![4]); // admin_email only

    // Test ends_with search for multiple matches: columns ending with 'e'
    let req = RequestBuilder::search_schema_text(
        "e",
        TextSearchType::EndsWith,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![0, 1, 5, 6, 7, 9]); // user_name, user_age, score, bonus_score, is_active, registration_date

    // Test case sensitivity: uppercase 'USER' with case_sensitive=true should match nothing
    let req = RequestBuilder::search_schema_text(
        "USER",
        TextSearchType::Contains,
        true,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![] as Vec<i64>);

    // Test case sensitivity: uppercase 'USER' with case_sensitive=false should match user columns
    let req = RequestBuilder::search_schema_text(
        "USER",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![0, 1, 2]);

    // Test not_contains search: columns that don't contain 'user'
    let req = RequestBuilder::search_schema_text(
        "user",
        TextSearchType::NotContains,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![3, 4, 5, 6, 7, 8, 9, 10]); // all except user_* columns

    // Test search with special characters: 'score' should match score and bonus_score
    let req = RequestBuilder::search_schema_text(
        "score",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![5, 6]); // score, bonus_score
}

#[test]
fn test_search_schema_data_type_filters() {
    let _lock = r_test_lock();
    let setup = TestDataBuilder::create_mixed_types_dataframe().unwrap();
    let socket = setup.socket();

    // Schema: name(0 - str), age(1 - int), score(2 - dbl), is_active(3 - lgl), date_joined(4 - Date)

    // Test filter for numeric columns: should match age (int) and score (dbl)
    let req = RequestBuilder::search_schema_data_types(
        vec![ColumnDisplayType::Number],
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![1, 2]);

    // Test filter for string columns: should match name only
    let req = RequestBuilder::search_schema_data_types(
        vec![ColumnDisplayType::String],
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![0]);

    // Test filter for boolean columns: should match is_active only
    let req = RequestBuilder::search_schema_data_types(
        vec![ColumnDisplayType::Boolean],
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![3]);

    // Test filter for date columns: should match date_joined only
    let req = RequestBuilder::search_schema_data_types(
        vec![ColumnDisplayType::Date],
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![4]);

    // Test filter for multiple data types: string and boolean
    let req = RequestBuilder::search_schema_data_types(
        vec![ColumnDisplayType::String, ColumnDisplayType::Boolean],
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![0, 3]); // name, is_active

    // Test filter for all numeric-like types: Number and Date
    let req = RequestBuilder::search_schema_data_types(
        vec![ColumnDisplayType::Number, ColumnDisplayType::Date],
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![1, 2, 4]); // age, score, date_joined

    // Test empty filter (should match nothing when no types specified)
    let req = RequestBuilder::search_schema_data_types(vec![], SearchSchemaSortOrder::Original);
    TestAssertions::assert_search_matches(socket, req, vec![] as Vec<i64>);
}

#[test]
fn test_search_schema_sort_orders() {
    let _lock = r_test_lock();
    let setup = TestDataBuilder::create_sort_order_test_dataframe().unwrap();
    let socket = setup.socket();

    // Test original sort order (no filters)
    let req = RequestBuilder::search_schema_with_filters(vec![], SearchSchemaSortOrder::Original);
    TestAssertions::assert_search_matches(socket, req, vec![0, 1, 2]);

    // Test ascending sort order
    let req =
        RequestBuilder::search_schema_with_filters(vec![], SearchSchemaSortOrder::AscendingName);
    TestAssertions::assert_search_matches(socket, req, vec![1, 2, 0]); // apple, banana, zebra

    // Test descending sort order
    let req =
        RequestBuilder::search_schema_with_filters(vec![], SearchSchemaSortOrder::DescendingName);
    TestAssertions::assert_search_matches(socket, req, vec![0, 2, 1]); // zebra, banana, apple
}

#[test]
fn test_search_schema_combined_filters() {
    let _lock = r_test_lock();
    let setup = TestSetup::from_expression(
        "data.frame(
            user_name = c('Alice', 'Bob'),
            user_age = c(25, 30),
            admin_name = c('Admin1', 'Admin2'),
            score = c(95.5, 87.2)
        )",
        None,
    )
    .unwrap();
    let socket = setup.socket();

    // Test combined filters: text contains 'user' AND data type is string
    let filters = vec![
        FilterBuilder::text_contains("user", false),
        FilterBuilder::match_data_types(vec![ColumnDisplayType::String]),
    ];
    let req = RequestBuilder::search_schema_with_filters(filters, SearchSchemaSortOrder::Original);
    TestAssertions::assert_search_matches(socket, req, vec![0]); // Only user_name matches both

    // Test text contains 'name' sorted descending
    let filters = vec![FilterBuilder::text_contains("name", false)];
    let req =
        RequestBuilder::search_schema_with_filters(filters, SearchSchemaSortOrder::DescendingName);
    TestAssertions::assert_search_matches(socket, req, vec![0, 2]); // user_name, admin_name
}

#[test]
fn test_search_schema_no_matches() {
    let _lock = r_test_lock();
    let setup = TestSetup::from_expression(
        "data.frame(name = c('Alice', 'Bob'), age = c(25, 30))",
        None,
    )
    .unwrap();

    // Test search with no matches
    let req = RequestBuilder::search_schema_text(
        "nonexistent",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(setup.socket(), req, vec![] as Vec<i64>);
}

#[test]
fn test_search_schema_type_sort_orders() {
    let _lock = r_test_lock();

    // Create a simpler dataframe with multiple columns of different types for type sorting tests
    let setup = TestSetup::from_expression(
        "data.frame(
            name = c('Alice', 'Bob'),           # str(0)
            is_active = c(TRUE, FALSE),         # lgl(1)
            age = c(25L, 30L),                 # int(2)
            height = c(170.5, 180.2),          # dbl(3)
            category = factor(c('A', 'B')),    # fct(4)
            description = c('desc1', 'desc2')  # str(5)
        )",
        None,
    )
    .unwrap();
    let socket = setup.socket();

    // Test ascending type sort order - should sort by lowercase type name
    // Schema has: str(0,5), lgl(1), int(2), dbl(3), fct(4)
    // Expected order: dbl columns, fct columns, int columns, lgl columns, str columns
    let req =
        RequestBuilder::search_schema_with_filters(vec![], SearchSchemaSortOrder::AscendingType);
    TestAssertions::assert_search_matches(socket, req, vec![3, 4, 2, 1, 0, 5]);

    // Test descending type sort order - should sort by lowercase type name in reverse
    // Expected order: str columns, lgl columns, int columns, fct columns, dbl columns
    let req =
        RequestBuilder::search_schema_with_filters(vec![], SearchSchemaSortOrder::DescendingType);
    TestAssertions::assert_search_matches(socket, req, vec![0, 5, 1, 2, 4, 3]);

    // Test type sorting with filters - only numeric types (int, dbl)
    let filters = vec![FilterBuilder::match_data_types(vec![
        ColumnDisplayType::Number,
    ])];

    // Ascending type sort with filter: dbl first, then int
    let req = RequestBuilder::search_schema_with_filters(
        filters.clone(),
        SearchSchemaSortOrder::AscendingType,
    );
    TestAssertions::assert_search_matches(socket, req, vec![3, 2]); // height (dbl), age (int)

    // Descending type sort with filter: int first, then dbl
    let req =
        RequestBuilder::search_schema_with_filters(filters, SearchSchemaSortOrder::DescendingType);
    TestAssertions::assert_search_matches(socket, req, vec![2, 3]); // age (int), height (dbl)

    // Test type sorting with boolean filter - single boolean column
    let bool_filters = vec![FilterBuilder::match_data_types(vec![
        ColumnDisplayType::Boolean,
    ])];

    let req = RequestBuilder::search_schema_with_filters(
        bool_filters,
        SearchSchemaSortOrder::AscendingType,
    );
    TestAssertions::assert_search_matches(socket, req, vec![1]); // is_active

    // Test edge case: type sorting with no matches
    let no_match_filters = vec![FilterBuilder::match_data_types(vec![
        ColumnDisplayType::Datetime,
    ])];

    let req = RequestBuilder::search_schema_with_filters(
        no_match_filters,
        SearchSchemaSortOrder::AscendingType,
    );
    TestAssertions::assert_search_matches(socket, req, vec![] as Vec<i64>); // No datetime columns

    // Test combined text and type filters with type sorting
    let combined_filters = vec![
        FilterBuilder::text_contains("a", false), // Contains letter 'a'
        FilterBuilder::match_data_types(vec![ColumnDisplayType::String]),
    ];

    let req = RequestBuilder::search_schema_with_filters(
        combined_filters,
        SearchSchemaSortOrder::AscendingType,
    );
    TestAssertions::assert_search_matches(socket, req, vec![4, 0]); // category, name (both str type and contain 'a')
}

#[test]
fn test_search_schema_text_with_sort_orders() {
    let _lock = r_test_lock();

    // Create a schema specifically for testing text search with sorting
    let setup = TestSetup::from_expression(
        "data.frame(
            zebra_name = c('z1', 'z2'),
            apple_name = c('a1', 'a2'),
            banana_score = c(1.0, 2.0),
            cherry_id = c(10L, 20L),
            date_value = c(100.5, 200.5)
        )",
        None,
    )
    .unwrap();
    let socket = setup.socket();

    // Schema: zebra_name(0), apple_name(1), banana_score(2), cherry_id(3), date_value(4)

    // Test text search with original order: 'name' should match zebra_name, apple_name
    let req = RequestBuilder::search_schema_text(
        "name",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![0, 1]); // original order

    // Test text search with ascending name sort: apple_name, zebra_name
    let req = RequestBuilder::search_schema_text(
        "name",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::AscendingName,
    );
    TestAssertions::assert_search_matches(socket, req, vec![1, 0]); // apple_name first

    // Test text search with descending name sort: zebra_name, apple_name
    let req = RequestBuilder::search_schema_text(
        "name",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::DescendingName,
    );
    TestAssertions::assert_search_matches(socket, req, vec![0, 1]); // zebra_name first

    // Test text search with type sort: both name columns are strings, so maintains relative order
    let req = RequestBuilder::search_schema_text(
        "name",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::AscendingType,
    );
    TestAssertions::assert_search_matches(socket, req, vec![0, 1]); // both str type, original relative order

    // Test broader search with type sorting: search for anything containing 'a'
    let req = RequestBuilder::search_schema_text(
        "a",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::AscendingType,
    );
    TestAssertions::assert_search_matches(socket, req, vec![2, 4, 0, 1]); // banana_score(dbl), date_value(dbl), zebra_name(str), apple_name(str)
}

#[test]
fn test_search_schema_edge_cases() {
    let _lock = r_test_lock();
    let setup = TestDataBuilder::create_mixed_types_dataframe().unwrap();
    let socket = setup.socket();

    // Test empty search term
    let req = RequestBuilder::search_schema_text(
        "",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![0, 1, 2, 3, 4]); // Empty string matches all columns

    // Test search term that matches nothing
    let req = RequestBuilder::search_schema_text(
        "xyz_nonexistent",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![] as Vec<i64>);

    // Test single character search
    let req = RequestBuilder::search_schema_text(
        "e",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![0, 1, 2, 3, 4]); // name, age, score, is_active, date_joined

    // Test underscore search (common separator)
    let req = RequestBuilder::search_schema_text(
        "_",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![3, 4]); // is_active, date_joined

    // Test case sensitivity with mixed case
    let setup = TestSetup::from_expression(
        "data.frame(UserName = c('test'), userName = c('test'), username = c('test'))",
        None,
    )
    .unwrap();
    let socket = setup.socket();

    // Case sensitive search: exact 'UserName' should match first column only
    let req = RequestBuilder::search_schema_text(
        "UserName",
        TextSearchType::Contains,
        true,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![0]); // Only exact case match

    // Case insensitive search should match all variations
    let req = RequestBuilder::search_schema_text(
        "username",
        TextSearchType::Contains,
        false,
        SearchSchemaSortOrder::Original,
    );
    TestAssertions::assert_search_matches(socket, req, vec![0, 1, 2]); // All variations
}

#[test]
fn test_column_labels() {
    let _lock = r_test_lock();

    // Create a data frame with column labels
    r_task(|| {
        harp::parse_eval_global(
            r#"
            df_with_labels <- data.frame(
                age = c(25, 30, 35),
                income = c(50000, 60000, 70000),
                score = c(85.5, 92.0, 88.5)
            )
            attr(df_with_labels$age, "label") <- "Age in years"
            attr(df_with_labels$income, "label") <- "Annual income (USD)"
            attr(df_with_labels$score, "label") <- "Test score percentage"
        "#,
        )
        .unwrap();
    });

    let setup = TestSetup::new("df_with_labels");
    let socket = setup.socket();

    // Get schema and verify column labels are present
    let req = RequestBuilder::get_schema(vec![0, 1, 2]);
    assert_match!(socket_rpc(socket, req),
        DataExplorerBackendReply::GetSchemaReply(schema) => {
            assert_eq!(schema.columns.len(), 3);

            // Check first column
            assert_eq!(schema.columns[0].column_name, "age");
            assert_eq!(schema.columns[0].column_label, Some("Age in years".to_string()));

            // Check second column
            assert_eq!(schema.columns[1].column_name, "income");
            assert_eq!(schema.columns[1].column_label, Some("Annual income (USD)".to_string()));

            // Check third column
            assert_eq!(schema.columns[2].column_name, "score");
            assert_eq!(schema.columns[2].column_label, Some("Test score percentage".to_string()));
        }
    );

    // Clean up
    r_task(|| {
        harp::parse_eval_global("rm(df_with_labels)").unwrap();
    });
}

#[test]
fn test_column_labels_missing() {
    let _lock = r_test_lock();

    // Create a data frame without column labels
    r_task(|| {
        harp::parse_eval_global(
            r#"
            df_no_labels <- data.frame(
                x = 1:3,
                y = 4:6,
                z = 7:9
            )
        "#,
        )
        .unwrap();
    });

    let setup = TestSetup::new("df_no_labels");
    let socket = setup.socket();

    // Get schema and verify column labels are None
    let req = RequestBuilder::get_schema(vec![0, 1, 2]);
    assert_match!(socket_rpc(socket, req),
        DataExplorerBackendReply::GetSchemaReply(schema) => {
            assert_eq!(schema.columns.len(), 3);

            // All columns should have no labels
            assert_eq!(schema.columns[0].column_name, "x");
            assert_eq!(schema.columns[0].column_label, None);

            assert_eq!(schema.columns[1].column_name, "y");
            assert_eq!(schema.columns[1].column_label, None);

            assert_eq!(schema.columns[2].column_name, "z");
            assert_eq!(schema.columns[2].column_label, None);
        }
    );

    // Clean up
    r_task(|| {
        harp::parse_eval_global("rm(df_no_labels)").unwrap();
    });
}

#[test]
fn test_column_labels_haven_compatibility() {
    let _lock = r_test_lock();

    // Test with haven::labelled vectors if haven is available
    r_task(|| {
        harp::parse_eval_global(
            r#"
            # Try to load haven; skip if not available
            if (require(haven, quietly = TRUE)) {
                df_haven <- data.frame(
                    basic = 1:3,
                    labelled_var = haven::labelled(c(1, 2, 3), label = "Labelled numeric variable")
                )
                # Also add a regular label attribute for comparison
                attr(df_haven$basic, "label") <- "Basic variable with regular label"
                haven_available <- TRUE
            } else {
                # Fallback: create a data frame that simulates haven::labelled behavior
                df_haven <- data.frame(
                    basic = 1:3,
                    labelled_var = c(1, 2, 3)
                )
                attr(df_haven$basic, "label") <- "Basic variable with regular label"
                attr(df_haven$labelled_var, "label") <- "Labelled numeric variable"
                class(df_haven$labelled_var) <- c("haven_labelled", "vctrs_vctr", "double")
                haven_available <- FALSE
            }
        "#,
        )
        .unwrap();
    });

    let setup = TestSetup::new("df_haven");
    let socket = setup.socket();

    // Get schema and verify column labels work with both regular and haven labelled columns
    let req = RequestBuilder::get_schema(vec![0, 1]);
    assert_match!(socket_rpc(socket, req),
        DataExplorerBackendReply::GetSchemaReply(schema) => {
            assert_eq!(schema.columns.len(), 2);

            // Check basic column with regular label
            assert_eq!(schema.columns[0].column_name, "basic");
            assert_eq!(schema.columns[0].column_label, Some("Basic variable with regular label".to_string()));

            // Check haven::labelled column
            assert_eq!(schema.columns[1].column_name, "labelled_var");
            assert_eq!(schema.columns[1].column_label, Some("Labelled numeric variable".to_string()));
        }
    );

    // Clean up
    r_task(|| {
        harp::parse_eval_global("rm(df_haven, haven_available)").unwrap();
    });
}

#[test]
fn test_column_labels_edge_cases() {
    let _lock = r_test_lock();

    // Test edge cases: empty labels, non-character labels, multiple labels, etc.
    r_task(|| {
        harp::parse_eval_global(
            r#"
            df_edge_cases <- data.frame(
                normal = c(1, 2, 3),
                empty_label = c(4, 5, 6),
                numeric_label = c(7, 8, 9),
                multiple_labels = c(10, 11, 12),
                null_label = c(13, 14, 15)
            )

            # Normal case
            attr(df_edge_cases$normal, "label") <- "Normal label"

            # Empty string label
            attr(df_edge_cases$empty_label, "label") <- ""

            # Numeric label (should be ignored/converted safely)
            attr(df_edge_cases$numeric_label, "label") <- 42

            # Multiple character labels (should take first one)
            attr(df_edge_cases$multiple_labels, "label") <- c("First label", "Second label")

            # NULL label (should result in None)
            attr(df_edge_cases$null_label, "label") <- NULL
        "#,
        )
        .unwrap();
    });

    let setup = TestSetup::new("df_edge_cases");
    let socket = setup.socket();

    // Get schema and verify edge cases are handled correctly
    let req = RequestBuilder::get_schema(vec![0, 1, 2, 3, 4]);
    assert_match!(socket_rpc(socket, req),
        DataExplorerBackendReply::GetSchemaReply(schema) => {
            assert_eq!(schema.columns.len(), 5);

            // Normal case
            assert_eq!(schema.columns[0].column_name, "normal");
            assert_eq!(schema.columns[0].column_label, Some("Normal label".to_string()));

            // Empty label should be treated as no label (None)
            assert_eq!(schema.columns[1].column_name, "empty_label");
            assert_eq!(schema.columns[1].column_label, None);

            // Numeric label should be ignored (None)
            assert_eq!(schema.columns[2].column_name, "numeric_label");
            assert_eq!(schema.columns[2].column_label, None);

            // Multiple labels should take the first one
            assert_eq!(schema.columns[3].column_name, "multiple_labels");
            assert_eq!(schema.columns[3].column_label, Some("First label".to_string()));

            // NULL label should be None
            assert_eq!(schema.columns[4].column_name, "null_label");
            assert_eq!(schema.columns[4].column_label, None);
        }
    );

    // Clean up
    r_task(|| {
        harp::parse_eval_global("rm(df_edge_cases)").unwrap();
    });
}

#[test]
fn test_export_with_sort_order() {
    // Create test data
    r_task(|| {
        harp::parse_eval_global(
            r#"
            df_sort_test <- data.frame(
                id = c(3, 1, 4, 2),
                value = c(30, 10, 40, 20),
                name = c("Charlie", "Alice", "David", "Bob")
            )
        "#,
        )
        .unwrap();
    });

    let setup = TestSetup::new("df_sort_test");
    let socket = setup.socket();

    // First, apply a sort by the 'value' column in ascending order
    let req = RequestBuilder::set_sort_columns(vec![ColumnSortKey {
        column_index: 1, // 'value' column
        ascending: true,
    }]);
    assert_match!(
        socket_rpc(socket, req),
        DataExplorerBackendReply::SetSortColumnsReply()
    );

    // Test 1: Export a single column (ColumnRange) - should respect sort order
    let req = DataExplorerBackendRequest::ExportDataSelection(ExportDataSelectionParams {
        selection: TableSelection {
            kind: TableSelectionKind::ColumnRange,
            selection: Selection::IndexRange(DataSelectionRange {
                first_index: 0,
                last_index: 0,
            }),
        },
        format: ExportFormat::Csv,
    });
    assert_match!(socket_rpc(socket, req),
        DataExplorerBackendReply::ExportDataSelectionReply(ExportedData { data, format }) => {
            assert_eq!(format, ExportFormat::Csv);
            // After sorting by value ascending, the id column should be: 1, 2, 3, 4
            assert_eq!(data, "id\n1\n2\n3\n4");
        }
    );

    // Test 2: Export multiple columns (ColumnIndices) - should respect sort order
    let req = DataExplorerBackendRequest::ExportDataSelection(ExportDataSelectionParams {
        selection: TableSelection {
            kind: TableSelectionKind::ColumnIndices,
            selection: Selection::Indices(DataSelectionIndices {
                indices: vec![0, 2], // id and name columns
            }),
        },
        format: ExportFormat::Csv,
    });
    assert_match!(socket_rpc(socket, req),
        DataExplorerBackendReply::ExportDataSelectionReply(ExportedData { data, format }) => {
            assert_eq!(format, ExportFormat::Csv);
            // After sorting by value ascending, should be: Alice, Bob, Charlie, David
            assert_eq!(data, "id,name\n1,Alice\n2,Bob\n3,Charlie\n4,David");
        }
    );

    // Test 3: Export all columns (ColumnRange) - should respect sort order
    let req = DataExplorerBackendRequest::ExportDataSelection(ExportDataSelectionParams {
        selection: TableSelection {
            kind: TableSelectionKind::ColumnRange,
            selection: Selection::IndexRange(DataSelectionRange {
                first_index: 0,
                last_index: 2,
            }),
        },
        format: ExportFormat::Csv,
    });
    assert_match!(socket_rpc(socket, req),
        DataExplorerBackendReply::ExportDataSelectionReply(ExportedData { data, format }) => {
            assert_eq!(format, ExportFormat::Csv);
            // After sorting by value ascending
            assert_eq!(data, "id,value,name\n1,10,Alice\n2,20,Bob\n3,30,Charlie\n4,40,David");
        }
    );

    // Now change to descending sort
    let req = RequestBuilder::set_sort_columns(vec![ColumnSortKey {
        column_index: 1, // 'value' column
        ascending: false,
    }]);
    assert_match!(
        socket_rpc(socket, req),
        DataExplorerBackendReply::SetSortColumnsReply()
    );

    // Test 4: Export with descending sort - should respect new sort order
    let req = DataExplorerBackendRequest::ExportDataSelection(ExportDataSelectionParams {
        selection: TableSelection {
            kind: TableSelectionKind::ColumnRange,
            selection: Selection::IndexRange(DataSelectionRange {
                first_index: 0,
                last_index: 0,
            }),
        },
        format: ExportFormat::Csv,
    });
    assert_match!(socket_rpc(socket, req),
        DataExplorerBackendReply::ExportDataSelectionReply(ExportedData { data, format }) => {
            assert_eq!(format, ExportFormat::Csv);
            // After sorting by value descending, the id column should be: 4, 3, 2, 1
            assert_eq!(data, "id\n4\n3\n2\n1");
        }
    );

    // Test 5: Apply multiple sort columns
    let req = RequestBuilder::set_sort_columns(vec![
        ColumnSortKey {
            column_index: 0, // 'id' column
            ascending: true,
        },
        ColumnSortKey {
            column_index: 2, // 'name' column as secondary sort
            ascending: false,
        },
    ]);
    assert_match!(
        socket_rpc(socket, req),
        DataExplorerBackendReply::SetSortColumnsReply()
    );

    // Test 6: Export with multiple sort columns
    let req = DataExplorerBackendRequest::ExportDataSelection(ExportDataSelectionParams {
        selection: TableSelection {
            kind: TableSelectionKind::ColumnIndices,
            selection: Selection::Indices(DataSelectionIndices {
                indices: vec![0, 2], // id and name columns
            }),
        },
        format: ExportFormat::Tsv, // Also test TSV format
    });
    assert_match!(socket_rpc(socket, req),
        DataExplorerBackendReply::ExportDataSelectionReply(ExportedData { data, format }) => {
            assert_eq!(format, ExportFormat::Tsv);
            // After sorting by id ascending (primary sort)
            assert_eq!(data, "id\tname\n1\tAlice\n2\tBob\n3\tCharlie\n4\tDavid");
        }
    );

    // Clean up
    r_task(|| {
        harp::parse_eval_global("rm(df_sort_test)").unwrap();
    });
}

#[test]
fn test_empty_data_frame_schema() {
    let _lock = r_test_lock();

    // Test schema behavior with 0-row data frames for different column types
    let socket = open_data_explorer_from_expression(
        "data.frame(
            a = numeric(0),
            b = character(0),
            c = logical(0),
            d = factor(character(0)),
            e = as.Date(character(0)),
            f = as.POSIXct(character(0))
        )",
        None,
    )
    .unwrap();

    let req = DataExplorerBackendRequest::GetSchema(GetSchemaParams {
        column_indices: vec![0, 1, 2, 3, 4, 5],
    });

    assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::GetSchemaReply(schema) => {
            assert_eq!(schema.columns.len(), 6);

            let expected_types = vec![
                (ColumnDisplayType::Number, "dbl"),
                (ColumnDisplayType::String, "str"),
                (ColumnDisplayType::Boolean, "lgl"),
                (ColumnDisplayType::String, "fct(0)"),
                (ColumnDisplayType::Date, "Date"),
                (ColumnDisplayType::Datetime, "POSIXct"),
            ];

            for (i, (expected_display, expected_name)) in expected_types.iter().enumerate() {
                assert_eq!(schema.columns[i].type_display, *expected_display);
                assert_eq!(schema.columns[i].type_name, expected_name.to_string());
                assert_eq!(schema.columns[i].column_name,
                    match i {
                        0 => "a",
                        1 => "b",
                        2 => "c",
                        3 => "d",
                        4 => "e",
                        5 => "f",
                        _ => unreachable!(),
                    }.to_string()
                );
            }
        }
    );
}

#[test]
fn test_empty_data_frame_data_values() {
    let _lock = r_test_lock();

    // Test data values request behavior with 0-row data frames
    let socket = open_data_explorer_from_expression(
        "data.frame(
            numbers = numeric(0),
            strings = character(0),
            booleans = logical(0)
        )",
        None,
    )
    .unwrap();

    let req = get_data_values_request(0, 10, vec![0, 1, 2], default_format_options());

    assert_match!(socket_rpc(&socket, req),
        DataExplorerBackendReply::GetDataValuesReply(data) => {
            assert_eq!(data.columns.len(), 3);
            // Each column should be empty
            for column in &data.columns {
                assert_eq!(column.len(), 0);
            }
        }
    );
}

#[test]
fn test_empty_data_frame_state() {
    let _lock = r_test_lock();

    // Test state request with 0-row data frame
    let socket = open_data_explorer_from_expression(
        "data.frame(x = numeric(0), y = character(0))",
        None,
    )
    .unwrap();

    assert_match!(socket_rpc(&socket, DataExplorerBackendRequest::GetState),
        DataExplorerBackendReply::GetStateReply(state) => {
            assert_eq!(state.table_shape.num_rows, 0);
            assert_eq!(state.table_shape.num_columns, 2);
            // has_row_labels depends on whether the data frame has row names, even when empty
            // For our empty data frame, this could be true or false depending on R's behavior
        }
    );
}

#[test]
fn test_empty_data_frame_column_profiles() {
    let _lock = r_test_lock();

    // Test column profile requests (histograms, summary stats) with 0-row data frames
    let socket = open_data_explorer_from_expression(
        "data.frame(numbers = numeric(0), strings = character(0))",
        None,
    )
    .unwrap();

    // Test histogram profile for empty numeric column
    let histogram_req = ProfileBuilder::small_histogram(0, ColumnHistogramParamsMethod::Fixed, 10, None);
    let req = RequestBuilder::get_column_profiles("empty_histogram".to_string(), vec![histogram_req]);

    expect_column_profile_results(&socket, req, |profiles| {
        let histogram = profiles[0].small_histogram.clone().unwrap();
        assert_eq!(histogram.bin_edges, Vec::<String>::new());
        assert_eq!(histogram.bin_counts, Vec::<i64>::new());
    });

    // Test frequency table for empty string column
    let freq_table_req = ProfileBuilder::small_frequency_table(1, 5);
    let req = RequestBuilder::get_column_profiles("empty_freq_table".to_string(), vec![freq_table_req]);

    expect_column_profile_results(&socket, req, |profiles| {
        let freq_table = profiles[0].small_frequency_table.clone().unwrap();
        assert_eq!(freq_table.values.len(), 0);
        assert_eq!(freq_table.counts.len(), 0);
        assert_eq!(freq_table.other_count, None);
    });
}

#[test]
fn test_single_row_data_frame_column_profiles() {
    let _lock = r_test_lock();

    // Test column profiles specifically for 1-row data frames to ensure sparklines work
    let socket = open_data_explorer_from_expression(
        "data.frame(
            single_num = c(42.5),
            single_str = c('hello'),
            single_bool = c(TRUE),
            single_int = c(7L)
        )",
        None,
    )
    .unwrap();

    // Test histogram profile for single value numeric column
    let histogram_req = ProfileBuilder::small_histogram(0, ColumnHistogramParamsMethod::Fixed, 10, None);
    let req = RequestBuilder::get_column_profiles("single_histogram".to_string(), vec![histogram_req]);

    expect_column_profile_results(&socket, req, |profiles| {
        let histogram = profiles[0].small_histogram.clone().unwrap();
        assert_eq!(histogram.bin_edges, vec!["42.50", "42.50"]);
        assert_eq!(histogram.bin_counts, vec![1]);
    });

    // Test frequency table for single value string column
    let freq_table_req = ProfileBuilder::small_frequency_table(1, 5);
    let req = RequestBuilder::get_column_profiles("single_freq_table".to_string(), vec![freq_table_req]);

    expect_column_profile_results(&socket, req, |profiles| {
        let freq_table = profiles[0].small_frequency_table.clone().unwrap();
        assert_eq!(freq_table.values.len(), 1);
        assert_eq!(freq_table.counts, vec![1]);
        assert_eq!(freq_table.other_count, None);
    });

    // Test different histogram methods work for single values
    let histogram_methods = vec![
        ColumnHistogramParamsMethod::Sturges,
        ColumnHistogramParamsMethod::FreedmanDiaconis,
        ColumnHistogramParamsMethod::Scott,
    ];

    for method in histogram_methods {
        let histogram_req = ProfileBuilder::small_histogram(3, method.clone(), 10, None); // single_int column
        let req = RequestBuilder::get_column_profiles(format!("single_histogram_{:?}", method), vec![histogram_req]);

        expect_column_profile_results(&socket, req, |profiles| {
            let histogram = profiles[0].small_histogram.clone().unwrap();
            assert_eq!(histogram.bin_edges, vec!["7", "7"]);
            assert_eq!(histogram.bin_counts, vec![1]);
        });
    }
}
