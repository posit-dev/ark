//
// r-data-explorer.rs
//
// Copyright (C) 2023-2024 by Posit Software, PBC
//
//

use std::cmp;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::data_explorer_comm::ColumnDisplayType;
use amalthea::comm::data_explorer_comm::ColumnProfileResult;
use amalthea::comm::data_explorer_comm::ColumnProfileType;
use amalthea::comm::data_explorer_comm::ColumnSchema;
use amalthea::comm::data_explorer_comm::ColumnSortKey;
use amalthea::comm::data_explorer_comm::DataExplorerBackendReply;
use amalthea::comm::data_explorer_comm::DataExplorerBackendRequest;
use amalthea::comm::data_explorer_comm::DataExplorerFrontendEvent;
use amalthea::comm::data_explorer_comm::GetColumnProfilesFeatures;
use amalthea::comm::data_explorer_comm::GetColumnProfilesParams;
use amalthea::comm::data_explorer_comm::GetDataValuesParams;
use amalthea::comm::data_explorer_comm::GetSchemaParams;
use amalthea::comm::data_explorer_comm::SchemaUpdateParams;
use amalthea::comm::data_explorer_comm::SearchSchemaFeatures;
use amalthea::comm::data_explorer_comm::SetRowFiltersFeatures;
use amalthea::comm::data_explorer_comm::SetRowFiltersParams;
use amalthea::comm::data_explorer_comm::SetSortColumnsParams;
use amalthea::comm::data_explorer_comm::SupportedFeatures;
use amalthea::comm::data_explorer_comm::TableData;
use amalthea::comm::data_explorer_comm::TableSchema;
use amalthea::comm::data_explorer_comm::TableShape;
use amalthea::comm::data_explorer_comm::TableState;
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
use harp::vector::formatted_vector::FormattedVector;
use harp::TableInfo;
use harp::TableKind;
use libr::*;
use serde::Deserialize;
use serde::Serialize;
use stdext::local;
use stdext::result::ResultOrLog;
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::interface::RMain;
use crate::lsp::events::EVENTS;
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

    /// The set of active row indices after all sorts and filters have been
    /// applied.
    row_indices: Vec<i32>,

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
    ) -> harp::Result<()> {
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
                    // Generate an initial set of row indices that are just the
                    // row numbers
                    let row_indices: Vec<i32> = if shape.num_rows < 1 {
                        vec![]
                    } else {
                        (1..=shape.num_rows).collect()
                    };

                    // Create the initial state for the data viewer
                    let viewer = Self {
                        title,
                        table: data,
                        binding,
                        shape,
                        row_indices,
                        sort_keys: vec![],
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

        Ok(())
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

            // Reset active row indices to be all rows
            self.row_indices = (1..=self.shape.num_rows).collect();

            // Clear active sort keys
            self.sort_keys.clear();

            DataExplorerFrontendEvent::SchemaUpdate(SchemaUpdateParams {
                discard_state: true,
            })
        } else {
            // Columns didn't change, but the data has. If there are sort
            // keys, we need to sort the rows again to reflect the new data.
            if self.sort_keys.len() > 0 {
                self.row_indices = r_task(|| self.r_sort_rows())?;
            }

            DataExplorerFrontendEvent::DataUpdate
        };

        self.comm
            .outgoing_tx
            .send(CommMsg::Data(serde_json::to_value(event)?))?;
        Ok(true)
    }

    fn handle_rpc(
        &mut self,
        req: DataExplorerBackendRequest,
    ) -> anyhow::Result<DataExplorerBackendReply> {
        match req {
            DataExplorerBackendRequest::GetSchema(GetSchemaParams {
                start_index,
                num_columns,
            }) => {
                // TODO: Support for data frames with over 2B rows. Note that neither base R nor
                // tidyverse support long vectors in data frames, but data.table does.
                let num_columns: i32 = num_columns.try_into()?;
                let start_index: i32 = start_index.try_into()?;
                self.get_schema(start_index, num_columns)
            },
            DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
                row_start_index,
                num_rows,
                column_indices,
            }) => {
                // TODO: Support for data frames with over 2B rows
                let row_start_index: i32 = row_start_index.try_into()?;
                let num_rows: i32 = num_rows.try_into()?;
                let column_indices: Vec<i32> = column_indices
                    .into_iter()
                    .map(i32::try_from)
                    .collect::<Result<Vec<i32>, _>>()?;
                r_task(|| self.r_get_data_values(row_start_index, num_rows, column_indices))
            },
            DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams {
                sort_keys: keys,
            }) => {
                // Save the new sort keys
                self.sort_keys = keys.clone();

                // If there are no sort keys, reset the row indices to be the
                // row numbers; otherwise, sort the rows
                self.row_indices = match keys.len() {
                    0 => (1..=self.shape.num_rows).collect(),
                    _ => r_task(|| self.r_sort_rows())?,
                };

                Ok(DataExplorerBackendReply::SetSortColumnsReply())
            },
            DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams { filters: _ }) => {
                bail!("Data Viewer: Not yet implemented")
            },
            DataExplorerBackendRequest::GetColumnProfiles(GetColumnProfilesParams {
                profiles: requests,
            }) => {
                let profiles = requests
                    .into_iter()
                    .map(|request| match request.profile_type {
                        ColumnProfileType::NullCount => {
                            let null_count =
                                r_task(|| self.r_null_count(request.column_index as i32));
                            ColumnProfileResult {
                                null_count: match null_count {
                                    Err(err) => {
                                        log::error!(
                                            "Error getting null count for column {}: {}",
                                            request.column_index,
                                            err
                                        );
                                        None
                                    },
                                    Ok(count) => Some(count as i64),
                                },
                                summary_stats: None,
                                histogram: None,
                                frequency_table: None,
                            }
                        },
                        _ => {
                            // Other kinds of column profiles are not yet
                            // implemented in R
                            ColumnProfileResult {
                                null_count: None,
                                summary_stats: None,
                                histogram: None,
                                frequency_table: None,
                            }
                        },
                    })
                    .collect::<Vec<ColumnProfileResult>>();
                Ok(DataExplorerBackendReply::GetColumnProfilesReply(profiles))
            },
            DataExplorerBackendRequest::GetState => r_task(|| self.r_get_state()),
            DataExplorerBackendRequest::GetSupportedFeatures => Ok(
                DataExplorerBackendReply::GetSupportedFeaturesReply(SupportedFeatures {
                    get_column_profiles: GetColumnProfilesFeatures {
                        supported: true,
                        supported_types: vec![ColumnProfileType::NullCount],
                    },
                    search_schema: SearchSchemaFeatures { supported: false },
                    set_row_filters: SetRowFiltersFeatures {
                        supported: false,
                        supported_types: vec![],
                        supports_conditions: false,
                    },
                }),
            ),
            DataExplorerBackendRequest::SearchSchema(_) => {
                bail!("Data Viewer: Not yet implemented")
            },
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

    /// Counts the number of nulls in a column. As the intent is to provide an
    /// idea of how complete the data is, NA values are considered to be null
    /// for the purposes of these stats.
    ///
    /// - `column_index`: The index of the column to count nulls in; 0-based.
    fn r_null_count(&self, column_index: i32) -> anyhow::Result<i32> {
        // Get the column to count nulls in
        let column = tbl_get_column(self.table.get().sexp, column_index, self.shape.kind)?;

        // Compute the number of nulls in the column
        let result = RFunction::new("", ".ps.null_count").add(column).call()?;

        // Return the count of nulls and NA values
        Ok(result.try_into()?)
    }

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
        order.param("decreasing", RObject::try_from(decreasing)?);
        order.param("method", RObject::from("radix"));

        // Invoke the order function and return the result
        let result = order.call()?;
        let indices: Vec<i32> = result.try_into()?;
        Ok(indices)
    }

    /// Get the schema for a range of columns in the data object.
    ///
    /// - `start_index`: The index of the first column to return.
    /// - `num_columns`: The number of columns to return.
    fn get_schema(
        &self,
        start_index: i32,
        num_columns: i32,
    ) -> anyhow::Result<DataExplorerBackendReply> {
        // Clip the range of columns requested to the actual number of columns
        // in the data object
        let total_num_columns = self.shape.columns.len() as i32;
        let lower_bound = cmp::min(start_index, total_num_columns);
        let upper_bound = cmp::min(total_num_columns, start_index + num_columns);

        // Return the schema for the requested columns
        let response = TableSchema {
            columns: self.shape.columns[lower_bound as usize..upper_bound as usize].to_vec(),
        };

        Ok(DataExplorerBackendReply::GetSchemaReply(response))
    }

    fn r_get_state(&self) -> anyhow::Result<DataExplorerBackendReply> {
        let table = self.table.get().clone();
        let object = *table;

        let harp::TableInfo {
            kind: _,
            dims:
                harp::TableDim {
                    num_rows,
                    num_cols: num_columns,
                },
            col_names: _,
        } = table_info_or_bail(object)?;

        let state = TableState {
            table_shape: TableShape {
                num_rows: num_rows.into(),
                num_columns: num_columns as i64,
            },
            row_filters: vec![],
            sort_keys: self.sort_keys.clone(),
        };
        Ok(DataExplorerBackendReply::GetStateReply(state))
    }

    fn r_get_data_values(
        &self,
        row_start_index: i32,
        num_rows: i32,
        column_indices: Vec<i32>,
    ) -> anyhow::Result<DataExplorerBackendReply> {
        let table = self.table.get().clone();
        let object = *table;

        let info = table_info_or_bail(object)?;

        let harp::TableInfo {
            dims:
                harp::TableDim {
                    num_rows: total_num_rows,
                    num_cols: total_num_cols,
                },
            ..
        } = info;

        let lower_bound = cmp::min(row_start_index, total_num_rows) as isize;
        let upper_bound = cmp::min(row_start_index + num_rows, total_num_rows) as isize;

        // Create R indices
        let cols_r_idx: Vec<i32> = column_indices
            .into_iter()
            // For now we skip any columns requested beyond last one
            .filter(|x| *x < total_num_cols)
            .map(|x| x + 1)
            .collect();
        let cols_r_idx: RObject = cols_r_idx.try_into()?;
        let num_cols = cols_r_idx.length() as i32;

        let row_indices = self.row_indices[lower_bound as usize..upper_bound as usize].to_vec();
        let rows_r_idx: RObject = row_indices.clone().try_into()?;

        // Subset rows in advance, including unmaterialized row names. Also
        // subset spend time creating subsetting columns that we don't need.
        // Supports dispatch and should be vectorised in most implementations.
        let object = RFunction::new("base", "[")
            .add(object)
            .add(rows_r_idx.sexp)
            .add(cols_r_idx.sexp)
            .param("drop", false)
            .call()?;

        let mut column_data: Vec<Vec<String>> = Vec::new();
        for i in 0..num_cols {
            let column = RFunction::new("base", "[")
                .add(object.clone())
                .add(unsafe { R_MissingArg })
                .add(i + 1)
                .param("drop", true)
                .call()?;

            let formatter = FormattedVector::new(*column)?;
            let formatted = formatter.iter().collect();

            column_data.push(formatted);
        }

        // Look for the row names attribute and include them if present
        // (if not, let the front end generate automatic row names)
        let row_names = object.attr("row.names");
        let row_labels = match row_names {
            Some(names) => match names.kind() {
                STRSXP => {
                    let labels: Vec<String> = names.try_into()?;
                    Some(vec![labels])
                },
                _ => {
                    // Create row names by using the row indices of the subset
                    // rows
                    let labels: Vec<String> = row_indices.iter().map(|x| x.to_string()).collect();
                    Some(vec![labels])
                },
            },
            None => None,
        };

        let response = TableData {
            columns: column_data,
            row_labels,
        };

        Ok(DataExplorerBackendReply::GetDataValuesReply(response))
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
