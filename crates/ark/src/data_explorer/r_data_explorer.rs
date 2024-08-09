//
// r-data-explorer.rs
//
// Copyright (C) 2023-2024 by Posit Software, PBC
//
//

use std::cmp;
use std::collections::HashMap;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::data_explorer_comm::ArraySelection;
use amalthea::comm::data_explorer_comm::BackendState;
use amalthea::comm::data_explorer_comm::ColumnDisplayType;
use amalthea::comm::data_explorer_comm::ColumnFilter;
use amalthea::comm::data_explorer_comm::ColumnProfileParams;
use amalthea::comm::data_explorer_comm::ColumnProfileRequest;
use amalthea::comm::data_explorer_comm::ColumnProfileResult;
use amalthea::comm::data_explorer_comm::ColumnProfileType;
use amalthea::comm::data_explorer_comm::ColumnProfileTypeSupportStatus;
use amalthea::comm::data_explorer_comm::ColumnSchema;
use amalthea::comm::data_explorer_comm::ColumnSelection;
use amalthea::comm::data_explorer_comm::ColumnSortKey;
use amalthea::comm::data_explorer_comm::ColumnSummaryStats;
use amalthea::comm::data_explorer_comm::ColumnValue;
use amalthea::comm::data_explorer_comm::DataExplorerBackendReply;
use amalthea::comm::data_explorer_comm::DataExplorerBackendRequest;
use amalthea::comm::data_explorer_comm::DataExplorerFrontendEvent;
use amalthea::comm::data_explorer_comm::ExportDataSelectionFeatures;
use amalthea::comm::data_explorer_comm::ExportDataSelectionParams;
use amalthea::comm::data_explorer_comm::ExportFormat;
use amalthea::comm::data_explorer_comm::ExportedData;
use amalthea::comm::data_explorer_comm::FilterComparisonOp;
use amalthea::comm::data_explorer_comm::FilterResult;
use amalthea::comm::data_explorer_comm::FormatOptions;
use amalthea::comm::data_explorer_comm::GetColumnProfilesFeatures;
use amalthea::comm::data_explorer_comm::GetColumnProfilesParams;
use amalthea::comm::data_explorer_comm::GetDataValuesParams;
use amalthea::comm::data_explorer_comm::GetSchemaParams;
use amalthea::comm::data_explorer_comm::RowFilter;
use amalthea::comm::data_explorer_comm::RowFilterParams;
use amalthea::comm::data_explorer_comm::RowFilterType;
use amalthea::comm::data_explorer_comm::RowFilterTypeSupportStatus;
use amalthea::comm::data_explorer_comm::SearchSchemaFeatures;
use amalthea::comm::data_explorer_comm::SetColumnFiltersFeatures;
use amalthea::comm::data_explorer_comm::SetRowFiltersFeatures;
use amalthea::comm::data_explorer_comm::SetRowFiltersParams;
use amalthea::comm::data_explorer_comm::SetSortColumnsFeatures;
use amalthea::comm::data_explorer_comm::SetSortColumnsParams;
use amalthea::comm::data_explorer_comm::SupportStatus;
use amalthea::comm::data_explorer_comm::SupportedFeatures;
use amalthea::comm::data_explorer_comm::TableData;
use amalthea::comm::data_explorer_comm::TableRowLabels;
use amalthea::comm::data_explorer_comm::TableSchema;
use amalthea::comm::data_explorer_comm::TableSelection;
use amalthea::comm::data_explorer_comm::TableShape;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use anyhow::anyhow;
use anyhow::bail;
use crossbeam::channel::unbounded;
use crossbeam::channel::Sender;
use crossbeam::select;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_symbol;
use harp::tbl_get_column;
use harp::utils::r_inherits;
use harp::utils::r_is_object;
use harp::utils::r_is_s4;
use harp::utils::r_typeof;
use harp::TableInfo;
use harp::TableKind;
use itertools::Itertools;
use libr::*;
use serde::Deserialize;
use serde::Serialize;
use stdext::local;
use stdext::result::ResultOrLog;
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::data_explorer::export_selection;
use crate::data_explorer::format;
use crate::data_explorer::format::format_string;
use crate::data_explorer::histogram::profile_frequency_table;
use crate::data_explorer::histogram::profile_histogram;
use crate::data_explorer::summary_stats::summary_stats;
use crate::data_explorer::utils::tbl_subset_with_view_indices;
use crate::interface::RMain;
use crate::lsp::events::EVENTS;
use crate::modules::ARK_ENVS;
use crate::r_task;
use crate::thread::RThreadSafe;
use crate::variables::variable::WorkspaceVariableDisplayType;

/// A name/value binding pair in an environment.
///
/// We use this to keep track of the data object that the data viewer is
/// currently viewing; when the binding changes, we update the data viewer
/// accordingly.
pub struct DataObjectEnvInfo {
    pub name: String,
    pub env: RThreadSafe<RObject>,
}

struct DataObjectShape {
    pub columns: Vec<ColumnSchema>,
    pub num_rows: i32,
    pub kind: TableKind,
}

/// The R backend for Positron's Data Explorer.
pub struct RDataExplorer {
    /// The human-readable title of the data viewer.
    title: String,

    /// The data object that the data viewer is currently viewing.
    table: RThreadSafe<RObject>,

    /// An optional binding to the environment containing the data object.
    /// This can be omitted for cases wherein the data object isn't in an
    /// environment (e.g. a temporary or unnamed object)
    binding: Option<DataObjectEnvInfo>,

    /// A cache containing the current number of rows and the schema for each
    /// column of the data object.
    shape: DataObjectShape,

    /// A cache containing the current set of sort keys.
    sort_keys: Vec<ColumnSortKey>,

    /// A cache containing the current set of row filters.
    row_filters: Vec<RowFilter>,

    /// A cache containing the current set of column filters
    col_filters: Vec<ColumnFilter>,

    /// The set of sorted row indices, if any sorts are applied. This always
    /// includes all row indices.
    sorted_indices: Option<Vec<i32>>,

    /// The set of filtered row indices, if any filters are applied. These are
    /// the row indices that remain after applying all row filters. They're
    /// sorted in ascending order.
    filtered_indices: Option<Vec<i32>>,

    /// When any sorts or filters are applied, the set of sorted and filtered
    /// row indices. This is the set of row indices that are displayed in the
    /// data viewer.
    view_indices: Option<Vec<i32>>,

    /// The communication socket for the data viewer.
    comm: CommSocket,

    /// A channel to send messages to the CommManager.
    comm_manager_tx: Sender<CommManagerEvent>,
}

#[derive(Deserialize, Serialize)]
struct Metadata {
    title: String,
}

impl RDataExplorer {
    pub fn start(
        title: String,
        data: RObject,
        binding: Option<DataObjectEnvInfo>,
        comm_manager_tx: Sender<CommManagerEvent>,
    ) -> harp::Result<String> {
        let id = Uuid::new_v4().to_string();

        let comm = CommSocket::new(
            CommInitiator::BackEnd,
            id.clone(),
            String::from("positron.dataExplorer"),
        );

        // To be able to `Send` the `data` to the thread to be owned by the data
        // viewer, it needs to be made thread safe
        let data = RThreadSafe::new(data);

        spawn!(format!("ark-data-viewer-{}-{}", title, id), move || {
            // Get the initial set of column schemas for the data object
            let shape = r_task(|| Self::r_get_shape(&data));
            match shape {
                // shape the columns; start the data viewer
                Ok(shape) => {
                    // Create the initial state for the data viewer
                    let viewer = Self {
                        title,
                        table: data,
                        binding,
                        shape,
                        sorted_indices: None,
                        filtered_indices: None,
                        view_indices: None,
                        sort_keys: vec![],
                        row_filters: vec![],
                        col_filters: vec![],
                        comm,
                        comm_manager_tx,
                    };

                    // Start the data viewer's execution thread
                    viewer.execution_thread();
                },
                Err(err) => {
                    // Didn't get the columns; log the error and close the comm
                    log::error!(
                        "Error retrieving initial object schema: '{}': {}",
                        title,
                        err
                    );

                    // Close the comm immediately since we can't proceed without
                    // the schema
                    comm_manager_tx
                        .send(CommManagerEvent::Closed(comm.comm_id))
                        .or_log_error("Error sending comm closed event")
                },
            }
        });

        Ok(id)
    }

    pub fn execution_thread(mut self) {
        let execute: anyhow::Result<()> = local! {
            let metadata = Metadata {
                title: self.title.clone(),
            };
            let comm_open_json = serde_json::to_value(metadata)?;
            // Notify frontend that the data viewer comm is open
            let event = CommManagerEvent::Opened(self.comm.clone(), comm_open_json);
            self.comm_manager_tx.send(event)?;
            Ok(())
        };

        if let Err(err) = execute {
            log::error!("Error while viewing object '{}': {}", self.title, err);
        };

        // Register a handler for console prompt events
        let (prompt_signal_tx, prompt_signal_rx) = unbounded::<()>();
        let listen_id = EVENTS.console_prompt.listen({
            move |_| {
                prompt_signal_tx.send(()).unwrap();
            }
        });

        // Flag initially set to false, but set to true if the user closes the
        // channel (i.e. the frontend is closed)
        let mut user_initiated_close = false;

        // Set up event loop to listen for incoming messages from the frontend
        loop {
            select! {
                // When a console prompt event is received, check for updates to
                // the underlying data
                recv(&prompt_signal_rx) -> msg => {
                    if let Ok(()) = msg {
                        match self.update() {
                            Ok(true) => {},
                            Ok(false) => {
                                // The binding has been removed (or replaced
                                // with something incompatible), so close the
                                // data viewer
                                break;
                            },
                            Err(err) => {
                                log::error!("Error while checking environment for data viewer update: {err}");
                            },
                        }
                    }
                },

                // When a message is received from the frontend, handle it
                recv(self.comm.incoming_rx) -> msg => {
                    let msg = unwrap!(msg, Err(e) => {
                        log::trace!("Data Viewer: Error while receiving message from frontend: {e:?}");
                        break;
                    });
                    log::info!("Data Viewer: Received message from frontend: {msg:?}");

                    // Break out of the loop if the frontend has closed the channel
                    if let CommMsg::Close = msg {
                        log::trace!("Data Viewer: Closing down after receiving comm_close from frontend.");

                        // Remember that the user initiated the close so that we can
                        // avoid sending a duplicate close message from the back end
                        user_initiated_close = true;
                        break;
                    }

                    let comm = self.comm.clone();
                    comm.handle_request(msg, |req| self.handle_rpc(req));
                }
            }
        }

        EVENTS.console_prompt.remove(listen_id);

        if !user_initiated_close {
            // Send a close message to the frontend if the frontend didn't
            // initiate the close
            self.comm.outgoing_tx.send(CommMsg::Close).unwrap();
        }
    }

    /// Check the environment bindings for updates to the underlying value
    ///
    /// Returns true if the update was processed; false if the binding has been
    /// removed and the data viewer should be closed.
    fn update(&mut self) -> anyhow::Result<bool> {
        // No need to check for updates if we have no binding
        if self.binding.is_none() {
            return Ok(true);
        }

        // See if the value has changed; this block returns a new value if it
        // has changed, or None if it hasn't
        let new = r_task(|| {
            let binding = self.binding.as_ref().unwrap();
            let env = binding.env.get().sexp;

            let new = unsafe {
                let sym = r_symbol!(binding.name);
                Rf_findVarInFrame(env, sym)
            };

            let old = self.table.get().sexp;
            if new == old {
                None
            } else {
                Some(RThreadSafe::new(unsafe { RObject::new(new) }))
            }
        });

        // No change to the value, so we're done
        if new.is_none() {
            return Ok(true);
        }

        // Update the value
        self.table = new.unwrap();

        // Now we need to check to see if the schema has changed or just a data
        // value. Regenerate the schema.
        //
        // Consider: there may be a cheaper way to test the schema for changes
        // than regenerating it, but it'd be a lot more complicated.
        let new_shape = match r_task(|| Self::r_get_shape(&self.table)) {
            Ok(shape) => shape,
            Err(_) => {
                // The most likely cause of this error is that the object is no
                // longer something with a usable shape -- it's been removed or
                // replaced with an object that doesn't work with the data
                // viewer (i.e. is non rectangular)
                return Ok(false);
            },
        };

        // Generate the appropriate event based on whether the schema has
        // changed
        let event = if self.shape.columns != new_shape.columns {
            // Columns changed, so update our cache, and we need to send a
            // schema update event
            self.shape = new_shape;

            // Update row filters to reflect the new schema
            self.row_filters_update()?;

            // Clear precomputed indices
            self.sorted_indices = None;
            self.filtered_indices = None;
            self.view_indices = None;

            // Clear active sort keys
            self.sort_keys.clear();

            // Recompute and apply filters and sorts.
            let (indices, _) = self.row_filters_compute()?;
            self.filtered_indices = indices;
            self.apply_sorts_and_filters();

            DataExplorerFrontendEvent::SchemaUpdate
        } else {
            // Columns didn't change, but the data has. If there are sort
            // keys, we need to sort the rows again to reflect the new data.
            if self.sort_keys.len() > 0 {
                self.sorted_indices = Some(r_task(|| self.r_sort_rows())?);
            }

            // Recompute and apply filters and sorts.
            let (indices, _) = self.row_filters_compute()?;
            self.filtered_indices = indices;
            self.apply_sorts_and_filters();

            DataExplorerFrontendEvent::DataUpdate
        };

        self.comm
            .outgoing_tx
            .send(CommMsg::Data(serde_json::to_value(event)?))?;
        Ok(true)
    }

    // Marks row_filters as invalid if the column no longer exists
    // If the column still exists, update the column schema of the filter
    // and check if they are still valid.
    // Should be called whenever there's a schema update, ie. when `self.shape` changes.
    fn row_filters_update(&mut self) -> anyhow::Result<()> {
        for rf in self.row_filters.iter_mut() {
            let new_schema = self
                .shape
                .columns
                .iter()
                .find(|c| c.column_name == rf.column_schema.column_name);

            match new_schema {
                Some(schema) => {
                    rf.column_schema = schema.clone();
                    let is_valid = Self::is_valid_filter(rf)?;
                    rf.is_valid = Some(is_valid);
                    rf.error_message = if is_valid {
                        None
                    } else {
                        Some("Unsupported column type for filter".to_string())
                    };
                },
                None => {
                    // the column no longer exists
                    rf.is_valid = Some(false);
                    rf.error_message = Some("Column was removed".to_string());
                },
            };
        }
        Ok(())
    }

    fn handle_rpc(
        &mut self,
        req: DataExplorerBackendRequest,
    ) -> anyhow::Result<DataExplorerBackendReply> {
        match req {
            DataExplorerBackendRequest::GetSchema(GetSchemaParams { column_indices }) => {
                self.get_schema(column_indices)
            },
            DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
                columns,
                format_options,
            }) => r_task(|| self.r_get_data_values(columns, format_options)),
            DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams {
                sort_keys: keys,
            }) => {
                // Save the new sort keys
                self.sort_keys = keys.clone();

                // If there are no sort keys, clear the precomputed sorted
                // indices; otherwise, sort the rows and save the result
                self.sorted_indices = match keys.len() {
                    0 => None,
                    _ => Some(r_task(|| self.r_sort_rows())?),
                };

                // Apply sorts to the filtered indices to create view indices
                self.apply_sorts_and_filters();

                Ok(DataExplorerBackendReply::SetSortColumnsReply())
            },
            DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams { filters }) => {
                // Save the new row filters
                self.row_filters = filters;

                // Compute the filtered indices
                let (indices, had_errors) = self.row_filters_compute()?;
                self.filtered_indices = indices;

                // Apply sorts to the filtered indices to create view indices
                self.apply_sorts_and_filters();

                Ok(DataExplorerBackendReply::SetRowFiltersReply({
                    FilterResult {
                        selected_num_rows: match self.filtered_indices {
                            Some(ref indices) => indices.len() as i64,
                            None => self.shape.num_rows as i64,
                        },
                        had_errors,
                    }
                }))
            },
            DataExplorerBackendRequest::GetColumnProfiles(GetColumnProfilesParams {
                profiles: requests,
                format_options,
            }) => {
                let profiles = requests
                    .into_iter()
                    .map(|request| r_task(|| self.r_get_column_profile(request, &format_options)))
                    .collect::<Vec<ColumnProfileResult>>();

                Ok(DataExplorerBackendReply::GetColumnProfilesReply(profiles))
            },
            DataExplorerBackendRequest::GetState => r_task(|| self.r_get_state()),
            DataExplorerBackendRequest::SearchSchema(_) => {
                return Err(anyhow!("Data Explorer: Not yet supported"));
            },
            DataExplorerBackendRequest::SetColumnFilters(_) => {
                return Err(anyhow!("Data Explorer: Not yet supported"));
            },
            DataExplorerBackendRequest::GetRowLabels(req) => {
                let row_labels =
                    r_task(|| self.r_get_row_labels(req.selection, &req.format_options))?;
                Ok(DataExplorerBackendReply::GetRowLabelsReply(
                    TableRowLabels {
                        row_labels: vec![row_labels],
                    },
                ))
            },
            DataExplorerBackendRequest::ExportDataSelection(ExportDataSelectionParams {
                selection,
                format,
            }) => Ok(DataExplorerBackendReply::ExportDataSelectionReply(
                ExportedData {
                    data: self.r_export_data_selection(selection, format.clone())?,
                    format,
                },
            )),
        }
    }
}

// Methods that must be run on the main R thread
impl RDataExplorer {
    fn r_get_shape(table: &RThreadSafe<RObject>) -> anyhow::Result<DataObjectShape> {
        unsafe {
            let table = table.get().clone();
            let object = *table;

            let info = table_info_or_bail(object)?;

            let harp::TableInfo {
                kind,
                dims:
                    harp::TableDim {
                        num_rows,
                        num_cols: total_num_columns,
                    },
                col_names: column_names,
            } = info;

            let mut column_schemas = Vec::<ColumnSchema>::new();
            for i in 0..(total_num_columns as isize) {
                let column_name = match column_names.get_unchecked(i) {
                    Some(name) => name,
                    None => format!("[, {}]", i + 1),
                };

                // TODO: handling for nested data frame columns

                let col = match kind {
                    harp::TableKind::Dataframe => VECTOR_ELT(object, i),
                    harp::TableKind::Matrix => object,
                };

                let type_name = WorkspaceVariableDisplayType::from(col, false).display_type;
                let type_display = display_type(col);

                column_schemas.push(ColumnSchema {
                    column_name,
                    column_index: i as i64,
                    type_name,
                    type_display,
                    description: None,
                    children: None,
                    precision: None,
                    scale: None,
                    timezone: None,
                    type_size: None,
                });
            }

            Ok(DataObjectShape {
                columns: column_schemas,
                kind,
                num_rows,
            })
        }
    }

    fn r_get_column_profile(
        &self,
        request: ColumnProfileRequest,
        format_options: &FormatOptions,
    ) -> ColumnProfileResult {
        let mut output = ColumnProfileResult {
            null_count: None,
            summary_stats: None,
            histogram: None,
            frequency_table: None,
        };

        let filtered_column = unwrap!(tbl_get_filtered_column(
            self.table.get(),
            request.column_index,
            &self.filtered_indices,
            self.shape.kind,
        ), Err(e) =>  {
            // In the case something goes wrong here we log the error and return an empty output.
            // This might still work for the other columns in the request.
            log::error!("Error applying filter indices for column: {}. Err: {e}", request.column_index);
            return output;
        });

        for profile_req in request.profiles {
            match profile_req.profile_type {
                ColumnProfileType::NullCount => {
                    let null_count = self.r_null_count(filtered_column.clone());
                    output.null_count = match null_count {
                        Err(err) => {
                            log::error!(
                                "Error getting null count for column {}: {}",
                                request.column_index,
                                err
                            );
                            None
                        },
                        Ok(count) => Some(count as i64),
                    };
                },
                ColumnProfileType::SummaryStats => {
                    let summary_stats =
                        self.r_summary_stats(filtered_column.clone(), &format_options);
                    output.summary_stats = match summary_stats {
                        Err(err) => {
                            log::error!(
                                "Error getting summary stats for column {}: {}",
                                request.column_index,
                                err
                            );
                            None
                        },
                        Ok(stats) => Some(stats),
                    };
                },
                ColumnProfileType::Histogram => {
                    let params = match profile_req.params {
                        None => Err("Missing parameters for the histogram"),
                        Some(par) => match par {
                            ColumnProfileParams::Histogram(p) => Ok(p),
                            _ => Err("Wrong type of parameters for the histogram."),
                        },
                    };

                    let params = unwrap!(params, Err(err) => {
                        log::error!(
                            "Unable to compute the histogram for column {}. Missing parameters {}",
                            request.column_index,
                            err
                        );
                        continue; // Go to next profile
                    });

                    let histogram =
                        profile_histogram(filtered_column.sexp, &params, &format_options);

                    output.histogram = match histogram {
                        Ok(hist) => Some(hist),
                        Err(err) => {
                            log::error!(
                                "Error getting histogram for column {}: {}",
                                request.column_index,
                                err
                            );
                            None
                        },
                    }
                },
                ColumnProfileType::FrequencyTable => {
                    let params = match profile_req.params {
                        None => Err("Missing parameters for the frequency table"),
                        Some(par) => match par {
                            ColumnProfileParams::FrequencyTable(p) => Ok(p),
                            _ => Err("Wrong type of parameters for the frequency table."),
                        },
                    };

                    let params = unwrap!(params, Err(err) => {
                        log::error!(
                            "Unable to compute the frequency table for column {}. Missing parameters {}",
                            request.column_index,
                            err
                        );
                        continue; // Go to next profile
                    });

                    let frequency_table =
                        profile_frequency_table(filtered_column.sexp, &params, &format_options);

                    output.frequency_table = match frequency_table {
                        Ok(hist) => Some(hist),
                        Err(err) => {
                            log::error!(
                                "Error getting frequency table for column {}: {}",
                                request.column_index,
                                err
                            );
                            None
                        },
                    }
                },
            };
        }
        output
    }

    /// Counts the number of nulls in a column. As the intent is to provide an
    /// idea of how complete the data is, NA values are considered to be null
    /// for the purposes of these stats.
    ///
    /// Expects data to be filtered by the view indices.
    ///
    /// - `column_index`: The index of the column to count nulls in; 0-based.
    fn r_null_count(&self, column: RObject) -> anyhow::Result<i32> {
        // Compute the number of nulls in the column
        let result = RFunction::new("", ".ps.null_count")
            .param("column", column)
            .call_in(ARK_ENVS.positron_ns)?;

        // Return the count of nulls and NA values
        Ok(result.try_into()?)
    }

    fn r_summary_stats(
        &self,
        column: RObject,
        format_options: &FormatOptions,
    ) -> anyhow::Result<ColumnSummaryStats> {
        // Get the column to compute summary stats for
        let dtype = display_type(column.sexp);
        Ok(summary_stats(column.sexp, dtype, format_options))
    }

    /// Sort the rows of the data object according to the sort keys in
    /// self.sort_keys.
    ///
    /// Returns a vector containing the sorted row indices.
    fn r_sort_rows(&self) -> anyhow::Result<Vec<i32>> {
        let mut order = RFunction::new("base", "order");

        // Allocate a vector to hold the sort order for each column
        let mut decreasing: Vec<bool> = Vec::new();

        // For each element of self.sort_keys, add an argument to order
        for key in &self.sort_keys {
            // Get the column to sort by
            order.add(tbl_get_column(
                self.table.get().sexp,
                key.column_index as i32,
                self.shape.kind,
            )?);
            decreasing.push(!key.ascending);
        }
        // Add the sort order per column
        order.param("decreasing", RObject::try_from(&decreasing)?);
        order.param("method", RObject::from("radix"));

        // Invoke the order function and return the result
        let result = order.call()?;
        let indices: Vec<i32> = result.try_into()?;
        Ok(indices)
    }

    /// Filter all the rows in the data object according to the row filters in
    /// self.row_filters.
    ///
    /// Returns a tuple containing a vector of all the row indices that pass the filters and
    /// a character vector of errors, where None means no error happened.
    fn r_filter_rows(&self) -> anyhow::Result<(Vec<i32>, Vec<Option<String>>)> {
        let mut filters: Vec<RObject> = vec![];

        // Shortcut: If there are no row filters, the filtered indices include
        // all row indices.
        if self.row_filters.is_empty() {
            return Ok(((1..=self.shape.num_rows).collect(), vec![]));
        }

        // Convert each filter to an R object by marshaling through the JSON
        // layer.
        //
        // This feels a little weird since the filters were *unmarshaled* from
        // JSON earlier in the RPC stack, but it's the easiest way to create R
        // objects from the filter data without creating an unnecessary
        // intermediate representation.
        for filter in &self.row_filters {
            let filter = serde_json::to_value(filter)?;
            let filter = RObject::try_from(filter)?;
            filters.push(filter);
        }

        // Pass the row filters to R and get the resulting row indices
        let filters = RObject::try_from(filters)?;
        let result: HashMap<String, RObject> = RFunction::new("", ".ps.filter_rows")
            .param("table", self.table.get().sexp)
            .param("row_filters", filters)
            .call_in(ARK_ENVS.positron_ns)?
            .try_into()?;

        // Handle errors that occured in the filters
        let row_indices = match result.get("indices") {
            Some(indices) => Vec::<i32>::try_from(indices.clone())?,
            None => bail!("Unexpected output from .ps.filter_rows. Expected 'indices' field."),
        };

        let errors = match result.get("errors") {
            Some(errors) => Vec::<Option<String>>::try_from(errors.clone())?,
            None => bail!("Unexpected output from .ps.filter_rows. Expected 'errors' field."),
        };

        Ok((row_indices, errors))
    }

    // Compute filtered indices out of the current `row_filters`.
    //
    // Implicitly updates the `row_filters` with validity status and error messages, if they
    // fail during the computation.
    fn row_filters_compute(&mut self) -> anyhow::Result<(Option<Vec<i32>>, Option<bool>)> {
        if self.row_filters.len() == 0 {
            return Ok((None, None));
        }

        let (indices, errors) = r_task(|| self.r_filter_rows())?;
        // this is called for the side-effect of updating the row_filters with validty status and
        // error messages
        let had_errors = Some(self.apply_filter_errors(errors)?);

        Ok((Some(indices), had_errors))
    }

    // Check if a filter is valid by looking at it's type and the type of the column its applied to.
    // Uses logic similar to python side: https://github.com/posit-dev/positron/blob/aafe313a261fd133b9f4a9f87c92bb10dc9966ad/extensions/positron-python/python_files/positron/positron_ipykernel/data_explorer.py#L743-L744
    fn is_valid_filter(filter: &RowFilter) -> anyhow::Result<bool> {
        let display_type = &filter.column_schema.type_display;
        let filter_type = &filter.filter_type;

        let is_compare_supported = |x: &ColumnDisplayType| match x {
            ColumnDisplayType::Number |
            ColumnDisplayType::Date |
            ColumnDisplayType::Datetime |
            ColumnDisplayType::Time => true,
            _ => false,
        };

        match filter_type {
            RowFilterType::IsEmpty | RowFilterType::NotEmpty | RowFilterType::Search => {
                // String-only filter types
                Ok(display_type == &ColumnDisplayType::String)
            },
            RowFilterType::Compare => {
                if let Some(params) = &filter.params {
                    match params {
                        RowFilterParams::Comparison(comparison) => match comparison.op {
                            FilterComparisonOp::Eq | FilterComparisonOp::NotEq => Ok(true),
                            _ => Ok(is_compare_supported(display_type)),
                        },
                        _ => Err(anyhow!("Missing compare filter params")),
                    }
                } else {
                    Err(anyhow!("Missing compare_params for filter"))
                }
            },
            RowFilterType::Between | RowFilterType::NotBetween => {
                Ok(is_compare_supported(display_type))
            },
            RowFilterType::IsTrue | RowFilterType::IsFalse => {
                Ok(display_type == &ColumnDisplayType::Boolean)
            },
            RowFilterType::IsNull | RowFilterType::NotNull | RowFilterType::SetMembership => {
                // Filters always supported
                Ok(true)
            },
        }
    }

    // Handle errors that occured in the filters
    //
    // This function mutates the `row_filters` attribute to include error messages and validity status.
    fn apply_filter_errors(&mut self, errors: Vec<Option<String>>) -> anyhow::Result<bool> {
        let mut had_errors = false;
        for (i, error) in errors.iter().enumerate() {
            match error {
                None => {
                    self.row_filters[i].is_valid = Some(true);
                },
                Some(error) => {
                    self.row_filters[i].is_valid = Some(false);
                    self.row_filters[i].error_message = Some(error.clone());
                    had_errors = true;
                },
            }
        }
        return Ok(had_errors);
    }

    /// Sort the filtered indices according to the sort keys, storing the
    /// result in view_indices.
    fn apply_sorts_and_filters(&mut self) {
        // If there are no filters or sorts, we don't need any view indices
        if self.filtered_indices.is_none() && self.sorted_indices.is_none() {
            self.view_indices = None;
            return;
        }

        // If there are filters but no sorts, the view indices are the filtered
        // indices
        if self.sorted_indices.is_none() {
            self.view_indices = self.filtered_indices.clone();
            return;
        }

        // If there are sorts but no filters, the view indices are the sorted
        // indices
        if self.filtered_indices.is_none() {
            self.view_indices = self.sorted_indices.clone();
            return;
        }

        // There are both sorts and filters, so we need to combine them.
        // self.sorted_indices contains all the indices; self.filtered_indices
        // contains the subset of indices that pass the filters, in ascending
        // order.
        //
        // Derive the set of indices that pass the filters and are sorted
        // according to the sort keys.
        let filtered_indices = self.filtered_indices.as_ref().unwrap();
        let sorted_indices = self.sorted_indices.as_ref().unwrap();
        let mut view_indices = Vec::<i32>::with_capacity(filtered_indices.len());
        for &index in sorted_indices {
            // We can use a binary search here for performance because
            // filtered_indices is already sorted in ascending order.
            if let Ok(_) = filtered_indices.binary_search(&index) {
                view_indices.push(index);
            }
        }
        self.view_indices = Some(view_indices);
    }

    /// Get the schema for a vector of columns in the data object.
    ///
    /// - `column_indices`: The vector of columns in the data object.
    fn get_schema(&self, column_indices: Vec<i64>) -> anyhow::Result<DataExplorerBackendReply> {
        // Get the columns length. (Does Rust optimize loop invariants well?)
        let columns_len = self.shape.columns.len();

        // Gather the column schemas to return.
        let mut columns: Vec<ColumnSchema> = Vec::new();
        for incoming_column_index in column_indices.into_iter().sorted() {
            // Validate that the incoming column index isn't negative.
            if incoming_column_index < 0 {
                return Err(anyhow!(
                    "Column index out of range {0}",
                    incoming_column_index
                ));
            }

            // Get the column index.
            let column_index = incoming_column_index as usize;

            // Break from the loop if the column index exceeds the number of columns.
            if column_index >= columns_len {
                break;
            }

            // Push the column schema.
            columns.push(self.shape.columns[column_index].clone());
        }

        // Return the table schema.
        Ok(DataExplorerBackendReply::GetSchemaReply(TableSchema {
            columns,
        }))
    }

    fn r_get_state(&self) -> anyhow::Result<DataExplorerBackendReply> {
        let state = BackendState {
            display_name: self.title.clone(),
            table_shape: TableShape {
                num_rows: match self.filtered_indices {
                    Some(ref indices) => indices.len() as i64,
                    None => self.shape.num_rows as i64,
                },
                num_columns: self.shape.columns.len() as i64,
            },
            table_unfiltered_shape: TableShape {
                num_rows: self.shape.num_rows as i64,
                num_columns: self.shape.columns.len() as i64,
            },
            row_filters: self.row_filters.clone(),
            column_filters: self.col_filters.clone(),
            sort_keys: self.sort_keys.clone(),
            has_row_labels: match self.table.get().attr("row.names") {
                Some(_) => true,
                None => false,
            },
            supported_features: SupportedFeatures {
                get_column_profiles: GetColumnProfilesFeatures {
                    support_status: SupportStatus::Supported,
                    supported_types: vec![
                        ColumnProfileTypeSupportStatus {
                            profile_type: ColumnProfileType::NullCount,
                            support_status: SupportStatus::Supported,
                        },
                        // Temporarily disabled for https://github.com/posit-dev/positron/issues/3490
                        // on 6/11/2024. This will be enabled again when the UI has been reworked to
                        // more fully support column profiles.
                        ColumnProfileTypeSupportStatus {
                            profile_type: ColumnProfileType::SummaryStats,
                            support_status: SupportStatus::Experimental,
                        },
                        ColumnProfileTypeSupportStatus {
                            profile_type: ColumnProfileType::Histogram,
                            support_status: SupportStatus::Unsupported,
                        },
                        ColumnProfileTypeSupportStatus {
                            profile_type: ColumnProfileType::FrequencyTable,
                            support_status: SupportStatus::Unsupported,
                        },
                    ],
                },
                search_schema: SearchSchemaFeatures {
                    support_status: SupportStatus::Unsupported,
                    supported_types: vec![],
                },
                set_row_filters: SetRowFiltersFeatures {
                    support_status: SupportStatus::Supported,
                    supported_types: vec![
                        RowFilterType::Between,
                        RowFilterType::Compare,
                        RowFilterType::IsEmpty,
                        RowFilterType::IsFalse,
                        RowFilterType::IsNull,
                        RowFilterType::IsTrue,
                        RowFilterType::NotBetween,
                        RowFilterType::NotEmpty,
                        RowFilterType::NotNull,
                        RowFilterType::Search,
                    ]
                    .iter()
                    .map(|row_filter_type| RowFilterTypeSupportStatus {
                        row_filter_type: row_filter_type.clone(),
                        support_status: SupportStatus::Supported,
                    })
                    .collect(),
                    // Temporarily disabled for https://github.com/posit-dev/positron/issues/3489
                    // on 6/11/2024. This will be enabled again when the UI has been reworked to
                    // support grouping.
                    supports_conditions: SupportStatus::Unsupported,
                },
                set_column_filters: SetColumnFiltersFeatures {
                    support_status: SupportStatus::Unsupported,
                    supported_types: vec![],
                },
                set_sort_columns: SetSortColumnsFeatures {
                    support_status: SupportStatus::Supported,
                },
                export_data_selection: ExportDataSelectionFeatures {
                    support_status: SupportStatus::Supported,
                    supported_formats: vec![
                        ExportFormat::Csv,
                        ExportFormat::Tsv,
                        ExportFormat::Html,
                    ],
                },
            },
        };
        Ok(DataExplorerBackendReply::GetStateReply(state))
    }

    fn r_get_data_values(
        &self,
        columns: Vec<ColumnSelection>,
        format_options: FormatOptions,
    ) -> anyhow::Result<DataExplorerBackendReply> {
        let mut column_data: Vec<Vec<ColumnValue>> = Vec::with_capacity(columns.len());
        for selection in columns {
            let tbl = tbl_subset_with_view_indices(
                self.table.get().sexp,
                &self.view_indices,
                Some(self.get_row_selection_indices(selection.spec)),
                Some(vec![selection.column_index]),
            )?;

            // The column will be always at index 0 because we already selected a single column above.
            let column = tbl_get_column(tbl.sexp, 0, self.shape.kind)?;
            let formatted = format::format_column(column.sexp, &format_options);
            column_data.push(formatted.clone());
        }

        let response = TableData {
            columns: column_data,
        };

        Ok(DataExplorerBackendReply::GetDataValuesReply(response))
    }

    fn r_get_row_labels(
        &self,
        selection: ArraySelection,
        format_options: &FormatOptions,
    ) -> anyhow::Result<Vec<String>> {
        let tbl = tbl_subset_with_view_indices(
            self.table.get().sexp,
            &self.view_indices,
            Some(self.get_row_selection_indices(selection)),
            Some(vec![]), // Use empty vec, because we only need the row names.
        )?;

        let row_names = RFunction::new("base", "row.names")
            .add(tbl)
            .call_in(ARK_ENVS.positron_ns)?;

        match row_names.kind() {
            STRSXP => {
                let labels = format_string(row_names.sexp, format_options);
                Ok(labels)
            },
            _ => {
                return Err(anyhow!(
                    "`row.names` should be strings, got {:?}",
                    row_names.kind()
                ))
            },
        }
    }

    // Given an ArraySelection, this materializes the indices that will actually be used.
    // Also does some sanity checks to avoid OOB access.
    fn get_row_selection_indices(&self, selection: ArraySelection) -> Vec<i64> {
        let num_view_rows = match self.view_indices {
            Some(ref indices) => indices.len() as i32,
            None => self.shape.num_rows,
        } as i64;

        // Returns the indices that will be collected
        match selection {
            ArraySelection::SelectRange(range) => {
                let lower_bound = cmp::min(range.first_index, num_view_rows);
                let upper_bound = cmp::min(range.last_index + 1, num_view_rows);
                (lower_bound..upper_bound).collect()
            },
            ArraySelection::SelectIndices(indices) => indices
                .indices
                .into_iter()
                .filter(|v| *v < num_view_rows)
                .collect(),
        }
    }

    fn r_export_data_selection(
        &self,
        selection: TableSelection,
        format: ExportFormat,
    ) -> anyhow::Result<String> {
        r_task(|| {
            export_selection::export_selection(
                self.table.get().sexp,
                &self.view_indices,
                selection,
                format,
            )
        })
    }
}

// This returns the type of an _element_ of the column. In R atomic
// vectors do not have a distinct internal type but we pretend that they
// do for the purpose of integrating with Positron types.
fn display_type(x: SEXP) -> ColumnDisplayType {
    if r_is_s4(x) {
        return ColumnDisplayType::Unknown;
    }

    if r_is_object(x) {
        if r_inherits(x, "logical") {
            return ColumnDisplayType::Boolean;
        }

        if r_inherits(x, "integer") {
            return ColumnDisplayType::Number;
        }
        if r_inherits(x, "double") {
            return ColumnDisplayType::Number;
        }
        if r_inherits(x, "complex") {
            return ColumnDisplayType::Number;
        }
        if r_inherits(x, "numeric") {
            return ColumnDisplayType::Number;
        }

        if r_inherits(x, "character") {
            return ColumnDisplayType::String;
        }
        if r_inherits(x, "factor") {
            return ColumnDisplayType::String;
        }

        if r_inherits(x, "Date") {
            return ColumnDisplayType::Date;
        }
        if r_inherits(x, "POSIXct") {
            return ColumnDisplayType::Datetime;
        }
        if r_inherits(x, "POSIXlt") {
            return ColumnDisplayType::Datetime;
        }

        // TODO: vctrs's list_of
        if r_inherits(x, "list") {
            return ColumnDisplayType::Unknown;
        }

        // Catch-all, including for data frame
        return ColumnDisplayType::Unknown;
    }

    match r_typeof(x) {
        LGLSXP => return ColumnDisplayType::Boolean,
        INTSXP | REALSXP | CPLXSXP => return ColumnDisplayType::Number,
        STRSXP => return ColumnDisplayType::String,
        VECSXP => return ColumnDisplayType::Unknown,
        _ => return ColumnDisplayType::Unknown,
    }
}

fn table_info_or_bail(x: SEXP) -> anyhow::Result<TableInfo> {
    harp::table_info(x).ok_or(anyhow!("Unsupported type for data viewer"))
}

fn tbl_get_filtered_column(
    x: &RObject,
    column_index: i64,
    indices: &Option<Vec<i32>>,
    kind: TableKind,
) -> anyhow::Result<RObject> {
    let column = tbl_get_column(x.sexp, column_index as i32, kind)?;

    Ok(match &indices {
        Some(indices) => RFunction::from("col_filter_indices")
            .add(column)
            .add(RObject::try_from(indices)?)
            .call_in(ARK_ENVS.positron_ns)?,
        None => column,
    })
}

/// Open an R object in the data viewer.
///
/// This function is called from the R side to open an R object in the data viewer.
///
/// # Parameters
/// - `x`: The R object to open in the data viewer.
/// - `title`: The title of the data viewer.
/// - `var`: The name of the variable containing the R object in its
///   environment; optional.
/// - `env`: The environment containing the R object; optional.
#[harp::register]
pub unsafe extern "C" fn ps_view_data_frame(
    x: SEXP,
    title: SEXP,
    var: SEXP,
    env: SEXP,
) -> anyhow::Result<SEXP> {
    let x = RObject::new(x);

    let title = RObject::new(title);
    let title = unwrap!(String::try_from(title), Err(_) => "".to_string());

    let main = RMain::get();
    let comm_manager_tx = main.get_comm_manager_tx().clone();

    // If an environment is provided, watch the variable in the environment
    let env_info = if env != R_NilValue {
        let var_obj = RObject::new(var);
        // Attempt to convert the variable name to a string
        match String::try_from(var_obj.clone()) {
            Ok(var_name) => Some(DataObjectEnvInfo {
                name: var_name,
                env: RThreadSafe::new(RObject::new(env)),
            }),
            Err(_) => {
                // If the variable name can't be converted to a string, don't
                // watch the variable.
                log::warn!(
                    "Attempt to watch variable in environment failed: {:?} not a string",
                    var_obj
                );
                None
            },
        }
    } else {
        None
    };

    RDataExplorer::start(title, x, env_info, comm_manager_tx)?;

    Ok(R_NilValue)
}
