//
// r-data-viewer.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommChannelMsg;
use amalthea::comm::event::CommEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use anyhow::bail;
use crossbeam::channel::Sender;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_lock;
use harp::utils::r_assert_length;
use harp::utils::r_is_data_frame;
use harp::utils::r_is_matrix;
use harp::utils::r_typeof;
use harp::vector::formatted_vector::FormattedVector;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use libR_sys::R_DimSymbol;
use libR_sys::R_MissingArg;
use libR_sys::R_NamesSymbol;
use libR_sys::R_RowNamesSymbol;
use libR_sys::Rf_getAttrib;
use libR_sys::INTEGER_ELT;
use libR_sys::SEXP;
use libR_sys::STRSXP;
use libR_sys::VECTOR_ELT;
use libR_sys::XLENGTH;
use serde::Deserialize;
use serde::Serialize;
use stdext::local;
use stdext::result::ResultExt;
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::data_viewer::message::DataViewerMessageRequest;
use crate::data_viewer::message::DataViewerMessageResponse;
use crate::data_viewer::message::DataViewerRowRequest;
use crate::data_viewer::message::DataViewerRowResponse;

pub struct RDataViewer {
    title: String,
    dataset: DataSet,
    comm: CommSocket,
    comm_manager_tx: Sender<CommEvent>,
}

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct DataColumn {
    pub name: String,

    #[serde(rename = "type")]
    pub column_type: String,

    pub data: Vec<String>,
}

impl DataColumn {
    fn slice(&self, start: usize, size: usize) -> Vec<String> {
        self.data[start..start + size].to_vec()
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

    pub fn from_object(id: String, title: String, object: RObject) -> Result<Self, anyhow::Error> {
        r_lock! {
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
                columns: columns,
                row_count: row_count
            })
        }
    }

    fn slice_data(&self, start: usize, size: usize) -> Result<Vec<DataColumn>, anyhow::Error> {
        const ZERO: usize = 0;
        if (start < ZERO) || (start >= self.row_count) {
            bail!("Invalid start row: {start}");
        } else if (start == ZERO) && (self.row_count <= size) {
            // No need to slice the data
            Ok(self.columns.clone())
        } else {
            let mut sliced_columns: Vec<DataColumn> = Vec::with_capacity(self.columns.len());
            for column in self.columns.iter() {
                sliced_columns.push(DataColumn {
                    name: column.name.clone(),
                    column_type: column.column_type.clone(),
                    data: column.slice(start, size),
                })
            }
            Ok(sliced_columns)
        }
    }
}

impl RDataViewer {
    pub fn start(title: String, data: RObject, comm_manager_tx: Sender<CommEvent>) {
        let id = Uuid::new_v4().to_string();

        let comm = CommSocket::new(
            CommInitiator::BackEnd,
            id.clone(),
            String::from("positron.dataViewer"),
        );

        // TODO: Don't preemptively format the full data set up front
        let dataset = unwrap!(
            DataSet::from_object(id.clone(), title.clone(), data), Err(error) => {
                log::error!("Data Viewer: Error while converting object to DataSet: {error}");
                return;
            }
        );

        spawn!(format!("ark-data-viewer-{}-{}", title, id), move || {
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
            let event = CommEvent::Opened(self.comm.clone(), comm_open_json);
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
            if msg == CommChannelMsg::Close {
                log::debug!("Data Viewer: Closing down after receiving comm_close from front end.");
                user_initiated_close = true;
                break;
            }

            // Process ordinary data messages
            if let CommChannelMsg::Rpc(id, data) = msg {
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
                .send(CommChannelMsg::Close)
                .on_err(|e| {
                    log::error!("Data Viewer: Failed to properly close the comm due to {e}.")
                });
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
            Some(id) => CommChannelMsg::Rpc(id, message.clone()),
            None => CommChannelMsg::Data(message.clone()),
        };
        self.comm
            .outgoing_tx
            .send(comm_msg)
            .on_err(|e| log::error!("Data Viewer: Failed to send message {message} due to: {e}."));
    }
}
