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
use harp::utils::r_is_data_frame;
use harp::utils::r_is_matrix;
use harp::utils::r_typeof;
use harp::vector::formatted_vector::FormattedVector;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use libr::R_DimSymbol;
use libr::R_MissingArg;
use libr::R_NamesSymbol;
use libr::R_NilValue;
use libr::Rf_getAttrib;
use libr::INTEGER_ELT;
use libr::SEXP;
use libr::STRSXP;
use libr::VECTOR_ELT;
use log::debug;
use log::error;
use serde::Deserialize;
use serde::Serialize;
use stdext::local;
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::interface::RMain;
use crate::r_task;
use crate::thread::RThreadSafe;
use crate::variables::variable::dim_data_frame;

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

struct ColumnNames {
    pub names: Option<CharacterVector>,
}

impl ColumnNames {
    pub fn new(names: SEXP) -> Self {
        unsafe {
            let names = if r_typeof(names) == STRSXP {
                Some(CharacterVector::new_unchecked(names))
            } else {
                None
            };
            Self { names }
        }
    }

    pub fn get_unchecked(&self, index: isize) -> Option<String> {
        if let Some(names) = &self.names {
            if let Some(name) = names.get_unchecked(index) {
                if name.len() > 0 {
                    return Some(name);
                }
            }
        }
        None
    }
}

impl RDataTool {
    pub fn start(
        title: String,
        data: RObject,
        comm_manager_tx: Sender<CommManagerEvent>,
    ) -> Result<(), harp::error::Error> {
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
        let execute: Result<(), anyhow::Error> = local! {
            let metadata = Metadata {
                title: self.title.clone(),
            };
            let comm_open_json = serde_json::to_value(metadata)?;
            // Notify frontend that the data viewer comm is open
            let event = CommManagerEvent::Opened(self.comm.clone(), comm_open_json);
            self.comm_manager_tx.send(event)?;
            Ok(())
        };

        if let Err(error) = execute {
            error!("Error while viewing object '{}': {}", self.title, error);
        };

        // Flag initially set to false, but set to true if the user closes the
        // channel (i.e. the frontend is closed)
        let mut user_initiated_close = false;

        // Set up event loop to listen for incoming messages from the frontend
        loop {
            let msg = unwrap!(self.comm.incoming_rx.recv(), Err(e) => {
                error!("Data Viewer: Error while receiving message from frontend: {e:?}");
                break;
            });
            debug!("Data Viewer: Received message from frontend: {msg:?}");

            // Break out of the loop if the frontend has closed the channel
            if let CommMsg::Close = msg {
                debug!("Data Viewer: Closing down after receiving comm_close from frontend.");

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
                let result = self.get_schema(start_index as i32, num_columns as i32)?;
                Ok(result)
            },
            DataToolBackendRequest::GetDataValues(GetDataValuesParams {
                row_start_index,
                num_rows,
                column_indices,
            }) => {
                // Fetch stringified data values and return
                let result = self.get_data_values(row_start_index, num_rows, column_indices)?;
                Ok(result)
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

    fn get_schema(
        &self,
        start_index: i32,
        num_columns: i32,
    ) -> Result<DataToolBackendReply, anyhow::Error> {
        r_task(|| unsafe {
            // TODO: Support for data frames with over 2B rows
            let table = self.table.get().clone();
            let object = *table;
            let num_rows: i64;
            let total_num_columns: i32;
            let column_names: ColumnNames;
            let is_data_frame = r_is_data_frame(object);
						let column_types: RObject;
            if is_data_frame {
                let dims = dim_data_frame(object);
                num_rows = dims.nrow as i64;
                total_num_columns = dims.ncol;
                column_names = ColumnNames::new(Rf_getAttrib(object, R_NamesSymbol));
                column_types = RFunction::from("sapply")
                    .add(object)
                    .add("class")
                    .call()?;
            } else if r_is_matrix(object) {
                let dim = Rf_getAttrib(object, R_DimSymbol);
                num_rows = INTEGER_ELT(dim, 0) as i64;
                total_num_columns = INTEGER_ELT(dim, 1);
                let colnames = RFunction::from("colnames").add(object).call()?;
                column_names = ColumnNames::new(*colnames);
                column_types = RFunction::from("typeof").add(object).call()?;
            } else {
                // TODO: better error message
                bail!("Unsupported type for data viewer");
            }

            let lower_bound = cmp::min(start_index, total_num_columns) as isize;
            let upper_bound = cmp::min(total_num_columns, start_index + num_columns) as isize;

            let mut column_schemas = Vec::<ColumnSchema>::new();
            for i in lower_bound..upper_bound {
                let column_name = match column_names.get_unchecked(i) {
                    Some(name) => name,
                    None => format!("[, {}]", i + 1),
                };

                // TODO: r_data_viewer.rs has some recursion code
                // that handles prefixes. is it possible that this
                // column is also a data frame?

                let type_name: String;
								if is_data_frame {
										let col_type = CharacterVector::new(VECTOR_ELT(*column_types, i))?;
										// TODO: some columns (e.g. temporal columns) can have multiple types
										type_name = match col_type.get_unchecked(0) {
												Some(value) => value,
												None => "unknown".to_string()
										};
								} else {
										type_name = match CharacterVector::new(*column_types)?.get_unchecked(0) {
												Some(value) => value,
												None => "unknown".to_string()
										};
								}

								let type_name = "unknown".to_string();

                let type_display = match column_name.as_str() {
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
                num_rows,
                total_num_columns: total_num_columns as i64,
            };

            Ok(DataToolBackendReply::GetSchemaReply(response))
        })
    }

    fn get_data_values(
        &self,
        row_start_index: i64,
        num_rows: i64,
        column_indices: Vec<i64>,
    ) -> Result<DataToolBackendReply, anyhow::Error> {
        r_task(|| unsafe {
            // TODO: Support for data frames with over 2B rows
            let table = self.table.get().clone();
            let object = *table;
            let total_num_rows: i64;
            let total_num_columns: i32;

            let is_data_frame = r_is_data_frame(object);

            if is_data_frame {
                let dims = dim_data_frame(object);
                total_num_rows = dims.nrow as i64;
                total_num_columns = dims.ncol;
            } else if r_is_matrix(object) {
                let dim = Rf_getAttrib(object, R_DimSymbol);
                total_num_rows = INTEGER_ELT(dim, 0) as i64;
                total_num_columns = INTEGER_ELT(dim, 1);
            } else {
                // TODO: better error message
                bail!("Unsupported type for data viewer");
            }

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
                if is_data_frame {
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
        })
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