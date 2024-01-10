//
// r-data-viewer.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use anyhow::bail;
use crossbeam::channel::Sender;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::utils::r_assert_length;
use harp::utils::r_is_data_frame;
use harp::utils::r_is_matrix;
use harp::utils::r_typeof;
use harp::vector::formatted_vector::FormattedVector;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use libR_shim::R_DimSymbol;
use libR_shim::R_MissingArg;
use libR_shim::R_NamesSymbol;
use libR_shim::R_NilValue;
use libR_shim::R_RowNamesSymbol;
use libR_shim::Rf_getAttrib;
use libR_shim::INTEGER_ELT;
use libR_shim::SEXP;
use libR_shim::STRSXP;
use libR_shim::VECTOR_ELT;
use libR_shim::XLENGTH;
use serde::Deserialize;
use serde::Serialize;
use stdext::local;
use stdext::result::ResultOrLog;
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::data_viewer::message::DataViewerMessageRequest;
use crate::data_viewer::message::DataViewerMessageResponse;
use crate::data_viewer::message::DataViewerRowRequest;
use crate::data_viewer::message::DataViewerRowResponse;
use crate::interface::RMain;
use crate::r_task;
use crate::thread::RThreadSafe;

pub struct RDataViewer {
    title: String,
    dataset: DataSet,
    comm: CommSocket,
    comm_manager_tx: Sender<CommManagerEvent>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct DataColumn {
    pub name: String,

    #[serde(rename = "type")]
    pub column_type: String,

    pub data: Vec<String>,
}

impl DataColumn {
    fn slice(&self, start: usize, end: usize) -> Vec<String> {
        self.data[start..end].to_vec()
    }
}

#[derive(Deserialize, Serialize, Debug)]
pub struct DataSet {
    pub id: String,
    pub title: String,
    pub columns: Vec<DataColumn>,

    #[serde(rename = "rowCount")]
    pub row_count: usize,
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

impl DataSet {
    unsafe fn extract_columns(
        object: SEXP,
        prefix: Option<String>,
        row_count: usize,
        columns: &mut Vec<DataColumn>,
    ) -> Result<(), anyhow::Error> {
        if r_is_data_frame(object) {
            unsafe {
                let names = ColumnNames::new(Rf_getAttrib(object, R_NamesSymbol));

                let n_columns = XLENGTH(object);
                for i in 0..n_columns {
                    let col_name = names.get_unchecked(i);

                    let name = match prefix {
                        None => match col_name {
                            Some(name) => name,
                            None => format!("[, {}]", i + 1),
                        },

                        Some(ref prefix) => match col_name {
                            Some(name) => format!("{}${}", prefix, name),
                            None => format!("{}[, {}]", prefix, i + 1),
                        },
                    };

                    // Protecting with `RObject` in case `object` happens to be an ALTLIST
                    let column = RObject::from(VECTOR_ELT(object, i));
                    Self::extract_columns(*column, Some(name), row_count, columns)?;
                }
            }
        } else if r_is_matrix(object) {
            unsafe {
                let dim = Rf_getAttrib(object, R_DimSymbol);
                let n_columns = INTEGER_ELT(dim, 1);
                let n_rows = INTEGER_ELT(dim, 0) as usize;
                if n_rows != row_count {
                    bail!("matrix column with incompatible number of rows");
                }

                let colnames = RFunction::from("colnames").add(object).call()?;
                let colnames = ColumnNames::new(*colnames);

                for i in 0..n_columns {
                    let col_name = colnames.get_unchecked(i as isize);

                    let name = match prefix {
                        None => match col_name {
                            Some(name) => name,
                            None => format!("[, {}]", i + 1),
                        },
                        Some(ref prefix) => match col_name {
                            Some(name) => format!("{}[, \"{}\"]", prefix, name),
                            None => format!("{}[, {}]", prefix, i + 1),
                        },
                    };

                    let matrix_column = RFunction::from("[")
                        .add(object)
                        .param("i", R_MissingArg)
                        .param("j", i + 1)
                        .call()?;

                    Self::extract_columns(*matrix_column, Some(name), row_count, columns)?;
                }
            }
        } else {
            r_assert_length(object, row_count)?;
            let data = FormattedVector::new(object)?.iter().collect();

            columns.push(DataColumn {
                name: prefix.unwrap(),

                // TODO: String here is a placeholder
                column_type: String::from("String"),
                data,
            });
        }

        Ok(())
    }

    unsafe fn from_object(
        id: String,
        title: String,
        object: RObject,
    ) -> Result<Self, anyhow::Error> {
        let row_count = {
            if r_is_data_frame(*object) {
                let row_names = Rf_getAttrib(*object, R_RowNamesSymbol);
                XLENGTH(row_names) as usize
            } else if r_is_matrix(*object) {
                let dim = Rf_getAttrib(*object, R_DimSymbol);
                INTEGER_ELT(dim, 0) as usize
            } else {
                bail!("data viewer only handles data frames and matrices")
            }
        };

        let mut columns = vec![];
        Self::extract_columns(*object, None, row_count, &mut columns)?;

        Ok(Self {
            id: id.clone(),
            title: title.clone(),
            columns,
            row_count,
        })
    }

    fn slice_data(&self, start: usize, size: usize) -> Result<Vec<DataColumn>, anyhow::Error> {
        const ZERO: usize = 0;

        if (start < ZERO) || (start >= self.row_count) {
            bail!("Invalid start row: {start}");
        }

        if (start == ZERO) && (self.row_count <= size) {
            // No need to slice the data
            return Ok(self.columns.clone());
        }

        let mut end = start + size; // exclusive, 1 more than the final row index of the slice

        if end > self.row_count {
            // Censor end to avoid a panic, but log an error as we expect the frontend
            // to handle this case already
            let row_count = self.row_count;
            log::error!("Requested rows [{start}, {end}), but only {row_count} rows exist.");
            end = self.row_count;
        }

        let mut sliced_columns: Vec<DataColumn> = Vec::with_capacity(self.columns.len());
        for column in self.columns.iter() {
            sliced_columns.push(DataColumn {
                name: column.name.clone(),
                column_type: column.column_type.clone(),
                data: column.slice(start, end),
            })
        }
        Ok(sliced_columns)
    }
}

impl RDataViewer {
    pub fn start(title: String, data: RObject, comm_manager_tx: Sender<CommManagerEvent>) {
        let id = Uuid::new_v4().to_string();

        let comm = CommSocket::new(
            CommInitiator::BackEnd,
            id.clone(),
            String::from("positron.dataViewer"),
        );

        // To be able to `Send` the `data` to the thread to be owned by the data
        // viewer, it needs to be made thread safe
        let data = RThreadSafe::new(data);

        spawn!(format!("ark-data-viewer-{}-{}", title, id), move || {
            let title_dataset = title.clone();

            // TODO: Don't preemptively format the full data set up front
            let dataset = r_task(move || {
                let data = data.get().clone();
                unsafe { DataSet::from_object(id, title_dataset, data) }
            });

            let dataset = unwrap!(dataset, Err(error) => {
                log::error!("Data Viewer: Error while converting object to DataSet: {error:?}");
                return;
            });

            let viewer = Self {
                title,
                dataset,
                comm,
                comm_manager_tx,
            };
            viewer.execution_thread();
        });
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
            log::error!("Error while viewing object '{}': {}", self.title, error);
        };

        // Flag initially set to false, but set to true if the user closes the
        // channel (i.e. the front end is closed)
        let mut user_initiated_close = false;

        // Set up event loop to listen for incoming messages from the frontend
        loop {
            let msg = unwrap!(self.comm.incoming_rx.recv(), Err(e) => {
                log::error!("Data Viewer: Error while receiving message from frontend: {e:?}");
                break;
            });
            log::debug!("Data Viewer: Received message from front end: {msg:?}");

            // Break out of the loop if the frontend has closed the channel
            if let CommMsg::Close = msg {
                log::debug!("Data Viewer: Closing down after receiving comm_close from front end.");
                user_initiated_close = true;
                break;
            }

            // Process ordinary data messages
            if let CommMsg::Rpc(id, data) = msg {
                let message = unwrap!(serde_json::from_value(data), Err(error) => {
                    log::error!("Data Viewer: Received invalid message from front end. {error}");
                    continue;
                });

                // Match on the type of message received
                match message {
                    DataViewerMessageRequest::Ready(DataViewerRowRequest {
                        start_row,
                        fetch_size,
                    }) => {
                        // Send the initial data (from 0 to fetch_size)
                        let message = unwrap!(
                            self.construct_response_message(start_row, fetch_size, true), Err(error) => {
                            log::error!("{error}");
                            break;
                        });
                        self.send_message(message, Some(id));
                    },

                    DataViewerMessageRequest::RequestRows(DataViewerRowRequest {
                        start_row,
                        fetch_size,
                    }) => {
                        // Send some additional data (from start_row to start_row + fetch_size)
                        let message = unwrap!(
                            self.construct_response_message(start_row, fetch_size, false), Err(error) => {
                            log::error!("{error}");
                            break;
                        });
                        self.send_message(message, Some(id));
                    },
                }
            }
        }

        if !user_initiated_close {
            // Send a close message to the front end if the front end didn't
            // initiate the close
            self.comm
                .outgoing_tx
                .send(CommMsg::Close)
                .or_log_error("Data Viewer: Failed to properly close the comm");
        }
    }

    fn construct_response_message(
        &self,
        start_row: usize,
        fetch_size: usize,
        initial: bool,
    ) -> Result<DataViewerMessageResponse, anyhow::Error> {
        let sliced_columns = unwrap!(self.dataset.slice_data(start_row, fetch_size), Err(e) => {
            if initial {
                bail!("Data Viewer: Failed to slice initial data: {e}");
            } else {
                bail!("Data Viewer: Failed to slice additional data at row {start_row}: {e}")
            }
        });

        let response_dataset = DataSet {
            id: self.dataset.id.clone(),
            title: self.title.clone(),
            columns: sliced_columns,
            row_count: self.dataset.row_count,
        };

        let response = DataViewerRowResponse {
            start_row,
            fetch_size,
            data: response_dataset,
        };

        let response = if initial {
            DataViewerMessageResponse::InitialData(response)
        } else {
            DataViewerMessageResponse::ReceiveRows(response)
        };
        Ok(response)
    }

    fn send_message(&self, message: DataViewerMessageResponse, request_id: Option<String>) {
        let message = unwrap!(serde_json::to_value(message), Err(err) => {
            log::error!("Data Viewer: Failed to serialize data viewer data: {}", err);
            return;
        });

        let comm_msg = match request_id {
            Some(id) => CommMsg::Rpc(id, message),
            None => CommMsg::Data(message),
        };
        self.comm
            .outgoing_tx
            .send(comm_msg)
            .or_log_error("Data Viewer: Failed to send message {message}");
    }
}

#[harp::register]
pub unsafe extern "C" fn ps_view_data_frame(x: SEXP, title: SEXP) -> anyhow::Result<SEXP> {
    let x = RObject::new(x);

    let title = RObject::new(title);
    let title = unwrap!(String::try_from(title), Err(_) => "".to_string());

    let main = RMain::get();
    let comm_manager_tx = main.get_comm_manager_tx().clone();

    RDataViewer::start(title, x, comm_manager_tx);

    Ok(R_NilValue)
}
