//
// r-data-viewer.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use std::cmp;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::data_explorer_comm::ColumnSchema;
use amalthea::comm::data_explorer_comm::ColumnSchemaTypeDisplay;
use amalthea::comm::data_explorer_comm::DataExplorerBackendReply;
use amalthea::comm::data_explorer_comm::DataExplorerBackendRequest;
use amalthea::comm::data_explorer_comm::GetColumnProfileParams;
use amalthea::comm::data_explorer_comm::GetDataValuesParams;
use amalthea::comm::data_explorer_comm::GetSchemaParams;
use amalthea::comm::data_explorer_comm::SetColumnFiltersParams;
use amalthea::comm::data_explorer_comm::SetSortColumnsParams;
use amalthea::comm::data_explorer_comm::TableData;
use amalthea::comm::data_explorer_comm::TableSchema;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use anyhow::bail;
use crossbeam::channel::Sender;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::utils::r_inherits;
use harp::utils::r_is_object;
use harp::utils::r_is_s4;
use harp::utils::r_typeof;
use harp::vector::formatted_vector::FormattedVector;
use libr::*;
use serde::Deserialize;
use serde::Serialize;
use stdext::local;
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::interface::RMain;
use crate::r_task;
use crate::thread::RThreadSafe;
use crate::variables::variable::WorkspaceVariableDisplayType;

pub struct RDataExplorer {
    title: String,
    table: RThreadSafe<RObject>,
    comm: CommSocket,
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
            let viewer = Self {
                title,
                table: data,
                comm,
                comm_manager_tx,
            };
            viewer.execution_thread();
        });

        Ok(())
    }

    pub fn execution_thread(self) {
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

        // Flag initially set to false, but set to true if the user closes the
        // channel (i.e. the frontend is closed)
        let mut user_initiated_close = false;

        // Set up event loop to listen for incoming messages from the frontend
        loop {
            let msg = unwrap!(self.comm.incoming_rx.recv(), Err(e) => {
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

            self.comm.handle_request(msg, |req| self.handle_rpc(req));
        }

        if !user_initiated_close {
            // Send a close message to the frontend if the frontend didn't
            // initiate the close
            self.comm.outgoing_tx.send(CommMsg::Close).unwrap();
        }
    }

    fn handle_rpc(
        &self,
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
                r_task(|| self.r_get_schema(start_index, num_columns))
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
            DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams { sort_keys: _ }) => {
                bail!("Data Viewer: Not yet implemented")
            },
            DataExplorerBackendRequest::SetColumnFilters(SetColumnFiltersParams { filters: _ }) => {
                bail!("Data Viewer: Not yet implemented")
            },
            DataExplorerBackendRequest::GetColumnProfile(GetColumnProfileParams {
                profile_type: _,
                column_index: _,
            }) => {
                bail!("Data Viewer: Not yet implemented")
            },
            DataExplorerBackendRequest::GetState => {
                bail!("Data Viewer: Not yet implemented")
            },
        }
    }
}

// Methods that must be run on the main R thread
impl RDataExplorer {
    fn r_get_schema(
        &self,
        start_index: i32,
        num_columns: i32,
    ) -> anyhow::Result<DataExplorerBackendReply> {
        unsafe {
            let table = self.table.get().clone();
            let object = *table;

            let harp::TableInfo {
                kind,
                dims:
                    harp::TableDim {
                        num_rows,
                        num_cols: total_num_columns,
                    },
                col_names: column_names,
            } = harp::table_info(object)?;

            let lower_bound = cmp::min(start_index, total_num_columns) as isize;
            let upper_bound = cmp::min(total_num_columns, start_index + num_columns) as isize;

            let mut column_schemas = Vec::<ColumnSchema>::new();
            for i in lower_bound..upper_bound {
                let column_name = match column_names.get_unchecked(i) {
                    Some(name) => name,
                    None => format!("[, {}]", i + 1),
                };

                // TODO: handling for nested data frame columns

                let col = match kind {
                    harp::TableKind::Dataframe => VECTOR_ELT(object, i),
                    harp::TableKind::Matrix => object,
                };

                let type_name = WorkspaceVariableDisplayType::from(col).display_type;
                let type_display = display_type(col);

                column_schemas.push(ColumnSchema {
                    column_name,
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

            let response = TableSchema {
                columns: column_schemas,
                num_rows: num_rows.into(),
                total_num_columns: total_num_columns as i64,
            };

            Ok(DataExplorerBackendReply::GetSchemaReply(response))
        }
    }

    fn r_get_data_values(
        &self,
        row_start_index: i32,
        num_rows: i32,
        column_indices: Vec<i32>,
    ) -> anyhow::Result<DataExplorerBackendReply> {
        let table = self.table.get().clone();
        let object = *table;

        let harp::TableInfo {
            dims:
                harp::TableDim {
                    num_rows: total_num_rows,
                    num_cols: total_num_cols,
                },
            ..
        } = harp::table_info(object)?;

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

        let rows_r_idx = RFunction::new("base", ":")
            .add((lower_bound + 1) as i32)
            .add((upper_bound + 1) as i32)
            .call()?;

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

        let row_names = RFunction::new("base", "row.names").add(object).call()?;
        let row_labels: Vec<String> = row_names.try_into()?;

        let response = TableData {
            columns: column_data,
            row_labels: Some(vec![row_labels]),
        };

        Ok(DataExplorerBackendReply::GetDataValuesReply(response))
    }
}

// This returns the type of an _element_ of the column. In R atomic
// vectors do not have a distinct internal type but we pretend that they
// do for the purpose of integrating with Positron types.
fn display_type(x: SEXP) -> ColumnSchemaTypeDisplay {
    if r_is_s4(x) {
        return ColumnSchemaTypeDisplay::Unknown;
    }

    if r_is_object(x) {
        if r_inherits(x, "logical") {
            return ColumnSchemaTypeDisplay::Boolean;
        }

        if r_inherits(x, "integer") {
            return ColumnSchemaTypeDisplay::Number;
        }
        if r_inherits(x, "double") {
            return ColumnSchemaTypeDisplay::Number;
        }
        if r_inherits(x, "complex") {
            return ColumnSchemaTypeDisplay::Number;
        }
        if r_inherits(x, "numeric") {
            return ColumnSchemaTypeDisplay::Number;
        }

        if r_inherits(x, "character") {
            return ColumnSchemaTypeDisplay::String;
        }
        if r_inherits(x, "factor") {
            return ColumnSchemaTypeDisplay::String;
        }

        if r_inherits(x, "Date") {
            return ColumnSchemaTypeDisplay::Date;
        }
        if r_inherits(x, "POSIXct") {
            return ColumnSchemaTypeDisplay::Datetime;
        }
        if r_inherits(x, "POSIXlt") {
            return ColumnSchemaTypeDisplay::Datetime;
        }

        // TODO: vctrs's list_of
        if r_inherits(x, "list") {
            return ColumnSchemaTypeDisplay::Unknown;
        }

        // Catch-all, including for data frame
        return ColumnSchemaTypeDisplay::Unknown;
    }

    match r_typeof(x) {
        LGLSXP => return ColumnSchemaTypeDisplay::Boolean,
        INTSXP | REALSXP | CPLXSXP => return ColumnSchemaTypeDisplay::Number,
        STRSXP => return ColumnSchemaTypeDisplay::String,
        VECSXP => return ColumnSchemaTypeDisplay::Unknown,
        _ => return ColumnSchemaTypeDisplay::Unknown,
    }
}

#[harp::register]
pub unsafe extern "C" fn ps_view_data_frame(x: SEXP, title: SEXP) -> anyhow::Result<SEXP> {
    let x = RObject::new(x);

    let title = RObject::new(title);
    let title = unwrap!(String::try_from(title), Err(_) => "".to_string());

    let main = RMain::get();
    let comm_manager_tx = main.get_comm_manager_tx().clone();

    RDataExplorer::start(title, x, comm_manager_tx)?;

    Ok(R_NilValue)
}
