//
// r-data-viewer.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use std::cmp;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::data_tool_comm::ColumnSchema;
use amalthea::comm::data_tool_comm::ColumnSchemaTypeDisplay;
use amalthea::comm::data_tool_comm::DataToolBackendReply;
use amalthea::comm::data_tool_comm::DataToolBackendRequest;
use amalthea::comm::data_tool_comm::GetColumnProfileParams;
use amalthea::comm::data_tool_comm::GetDataValuesParams;
use amalthea::comm::data_tool_comm::GetSchemaParams;
use amalthea::comm::data_tool_comm::SetColumnFiltersParams;
use amalthea::comm::data_tool_comm::SetSortColumnsParams;
use amalthea::comm::data_tool_comm::TableData;
use amalthea::comm::data_tool_comm::TableSchema;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use anyhow::bail;
use crossbeam::channel::Sender;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::vector::formatted_vector::FormattedVector;
use libr::R_MissingArg;
use libr::R_NilValue;
use libr::SEXP;
use libr::VECTOR_ELT;
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

pub struct RDataTool {
    title: String,
    table: RThreadSafe<RObject>,
    comm: CommSocket,
    comm_manager_tx: Sender<CommManagerEvent>,
}

#[derive(Deserialize, Serialize)]
struct Metadata {
    title: String,
}

impl RDataTool {
    pub fn start(
        title: String,
        data: RObject,
        comm_manager_tx: Sender<CommManagerEvent>,
    ) -> harp::Result<()> {
        let id = Uuid::new_v4().to_string();

        let comm = CommSocket::new(
            CommInitiator::BackEnd,
            id.clone(),
            String::from("positron.dataTool"),
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

    fn handle_rpc(&self, req: DataToolBackendRequest) -> anyhow::Result<DataToolBackendReply> {
        match req {
            DataToolBackendRequest::GetSchema(GetSchemaParams {
                start_index,
                num_columns,
            }) => {
                // TODO: Support for data frames with over 2B rows
                // TODO: Check bounds
                r_task(|| self.r_get_schema(start_index as i32, num_columns as i32))
            },
            DataToolBackendRequest::GetDataValues(GetDataValuesParams {
                row_start_index,
                num_rows,
                column_indices,
            }) => {
                // Fetch stringified data values and return
                r_task(|| self.r_get_data_values(row_start_index, num_rows, column_indices))
            },
            DataToolBackendRequest::SetSortColumns(SetSortColumnsParams { sort_keys: _ }) => {
                bail!("Data Viewer: Not yet implemented")
            },
            DataToolBackendRequest::SetColumnFilters(SetColumnFiltersParams { filters: _ }) => {
                bail!("Data Viewer: Not yet implemented")
            },
            DataToolBackendRequest::GetColumnProfile(GetColumnProfileParams {
                profile_type: _,
                column_index: _,
            }) => {
                bail!("Data Viewer: Not yet implemented")
            },
            DataToolBackendRequest::GetState => {
                bail!("Data Viewer: Not yet implemented")
            },
        }
    }
}

// Methods that must be run on the main R thread
impl RDataTool {
    fn r_get_schema(
        &self,
        start_index: i32,
        num_columns: i32,
    ) -> anyhow::Result<DataToolBackendReply> {
        unsafe {
            let table = self.table.get().clone();
            let object = *table;

            let harp::TableInfo {
                kind,
                num_rows,
                num_cols: total_num_columns,
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

                let col_type;
                if let harp::TableKind::Dataframe = kind {
                    col_type = WorkspaceVariableDisplayType::from(VECTOR_ELT(object, i));
                } else {
                    col_type = WorkspaceVariableDisplayType::from(object);
                }

                // TODO: this doesn't work because display_type has
                // the size added to it, like str [4]

                let type_display = match col_type.display_type.as_str() {
                    "character" => ColumnSchemaTypeDisplay::String,
                    "complex" => ColumnSchemaTypeDisplay::Number,
                    "integer" => ColumnSchemaTypeDisplay::Number,
                    "numeric" => ColumnSchemaTypeDisplay::Number,
                    "list" => ColumnSchemaTypeDisplay::Array,
                    "logical" => ColumnSchemaTypeDisplay::Boolean,
                    "Date" => ColumnSchemaTypeDisplay::Date,
                    "POSIXct" => ColumnSchemaTypeDisplay::Datetime,
                    _ => ColumnSchemaTypeDisplay::Unknown,
                };

                column_schemas.push(ColumnSchema {
                    column_name,
                    type_name: col_type.display_type,
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
                num_rows,
                total_num_columns: total_num_columns as i64,
            };

            Ok(DataToolBackendReply::GetSchemaReply(response))
        }
    }

    fn r_get_data_values(
        &self,
        row_start_index: i64,
        num_rows: i64,
        column_indices: Vec<i64>,
    ) -> anyhow::Result<DataToolBackendReply> {
        unsafe {
            let table = self.table.get().clone();
            let object = *table;

            let harp::TableInfo {
                kind,
                num_rows: total_num_rows,
                num_cols: total_num_columns,
                ..
            } = harp::table_info(object)?;

            let lower_bound = cmp::min(row_start_index, total_num_rows) as isize;
            let upper_bound = cmp::min(row_start_index + num_rows, total_num_rows) as isize;
            let mut column_data: Vec<Vec<String>> = Vec::new();
            for column_index in column_indices {
                let column_index = column_index as i32;
                if column_index >= total_num_columns {
                    // For now we skip any columns requested beyond last one
                    break;
                }

                let formatter: FormattedVector;
                let column: RObject;

                if let harp::TableKind::Dataframe = kind {
                    column = RObject::from(VECTOR_ELT(object, column_index as isize));
                } else {
                    column = RFunction::from("[")
                        .add(object)
                        .param("i", R_MissingArg)
                        .param("j", column_index + 1)
                        .call()?;
                }
                formatter = FormattedVector::new(*column)?;

                let mut formatted_data = Vec::new();
                for i in lower_bound..upper_bound {
                    formatted_data.push(formatter.get_unchecked(i));
                }
                column_data.push(formatted_data);
            }

            let response = TableData {
                columns: column_data,
                row_labels: Some(vec![]),
            };

            Ok(DataToolBackendReply::GetDataValuesReply(response))
        }
    }
}

#[harp::register]
pub unsafe extern "C" fn ps_view_data_frame(x: SEXP, title: SEXP) -> anyhow::Result<SEXP> {
    let x = RObject::new(x);

    let title = RObject::new(title);
    let title = unwrap!(String::try_from(title), Err(_) => "".to_string());

    let main = RMain::get();
    let comm_manager_tx = main.get_comm_manager_tx().clone();

    RDataTool::start(title, x, comm_manager_tx)?;

    Ok(R_NilValue)
}
