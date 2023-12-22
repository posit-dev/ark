//
// connection.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use crossbeam::channel::Sender;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use libR_shim::*;
use serde::Deserialize;
use serde::Serialize;
use stdext::result::ResultOrLog;
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::interface::RMain;
use crate::r_task;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum ConnectionResponse {
    TablesResponse { name: String, tables: Vec<String> },
    FieldsResponse { name: String, fields: Vec<String> },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum ConnectionRequest {
    // The UI is asking for the list of tables in the connection.
    TablesRequest,
    // The UI is asking for the list of fields in a table.
    FieldsRequest { table: String },
    // The UI asks for a DataViewer preview of the table.
    PreviewTable { table: String },
}

#[derive(Deserialize, Serialize)]
struct Metadata {
    name: String,
}

struct RConnection {
    name: String,
    comm: CommSocket,
    comm_manager_tx: Sender<CommManagerEvent>,
}

impl RConnection {
    fn start(
        name: String,
        comm_manager_tx: Sender<CommManagerEvent>,
    ) -> Result<String, anyhow::Error> {
        let id = Uuid::new_v4().to_string();

        let comm = CommSocket::new(
            CommInitiator::BackEnd,
            id.clone(),
            String::from("positron.connection"),
        );

        let connection = Self {
            name,
            comm,
            comm_manager_tx,
        };

        log::info!("Connection Pane: Channel created id:{id}");
        connection.open_and_register_comm()?;

        spawn!(format!("ark-connection-{}", id), move || {
            unwrap!(connection.handle_messages(), Err(e) => {
                log::error!("Connection Pane: Error while handling messages: {e:?}");
            });
        });

        Ok(id)
    }

    fn open_and_register_comm(&self) -> Result<(), anyhow::Error> {
        let metadata = Metadata {
            name: self.name.clone(),
        };
        let comm_open_json = serde_json::to_value(metadata)?;

        // Notify the frontend that a new connection has been opened.
        let event = CommManagerEvent::Opened(self.comm.clone(), comm_open_json);
        self.comm_manager_tx.send(event)?;

        Ok(())
    }

    fn handle_messages(&self) -> Result<(), anyhow::Error> {
        loop {
            let msg = unwrap!(self.comm.incoming_rx.recv(), Err(e) => {
                log::error!("Connection Pane: Error while receiving message from frontend: {e:?}");
                break; // TODO: Should we continue here? What if the frontend is still alive?
            });

            log::debug!("Connection Pane: Received message from front end: {msg:?}");

            if msg == CommMsg::Close {
                log::debug!("Connection Pane: Received a close message.");
                break;
            }

            if let CommMsg::Rpc(id, data) = msg {
                let message = unwrap!(serde_json::from_value(data), Err(error) => {
                    log::error!("Connection Pane: Received invalid message from front end. {error}");
                    continue;
                });

                match message {
                    ConnectionRequest::TablesRequest => {
                        let tables = r_task(|| unsafe {
                            // returns a data.frame with columns name and type
                            let tables = RFunction::from(".ps.connection_list_objects")
                                .add(RObject::from(self.comm.comm_id.clone()))
                                .param("catalog", "SQLiteConnection")
                                .param("schema", "Default")
                                .call()?;

                            // for now we only need the name column
                            let names = RFunction::from("[[")
                                .add(tables)
                                .add(RObject::from("name"))
                                .call()?;

                            RObject::to::<Vec<String>>(names)
                        })?;

                        let msg = ConnectionResponse::TablesResponse {
                            name: self.name.clone(),
                            tables,
                        };
                        self.send_message(msg, Some(id));
                    },
                    ConnectionRequest::FieldsRequest { table } => {
                        let fields = r_task(|| unsafe {
                            let fields = RFunction::from(".ps.connection_list_fields")
                                .add(RObject::from(self.comm.comm_id.clone()))
                                .param("catalog", "SQLiteConnection")
                                .param("schema", "Default")
                                .param("table", table)
                                .call()?;

                            // for now we only need the name column
                            let names = RFunction::from("[[")
                                .add(fields)
                                .add(RObject::from("name"))
                                .call()?;

                            RObject::to::<Vec<String>>(names)
                        })?;

                        let msg = ConnectionResponse::FieldsResponse {
                            name: self.name.clone(),
                            fields,
                        };
                        self.send_message(msg, Some(id));
                    },
                    ConnectionRequest::PreviewTable { table } => {
                        // Calls back into R to get the preview data.
                        r_task(|| -> Result<(), anyhow::Error> {
                            RFunction::from(".ps.connection_preview_object")
                                .add(RObject::from(self.comm.comm_id.clone()))
                                .param("catalog", "SQLiteConnection")
                                .param("schema", "Default")
                                .param("table", table)
                                .call()?;
                            Ok(())
                        })?;
                    },
                }
            }
        }

        // before finalizing the thread we make sure to send a close message to the front end
        if let Err(e) = self.comm.outgoing_tx.send(CommMsg::Close) {
            log::error!("Connection Pane: Error while sending comm_close to front end: {e:?}");
        }

        Ok(())
    }

    fn send_message(&self, message: ConnectionResponse, request_id: Option<String>) {
        let message = unwrap!(serde_json::to_value(message), Err(err) => {
            log::error!("Connection Pane: Failed to serialize data: {}", err);
            return;
        });

        let comm_msg = match request_id {
            Some(id) => CommMsg::Rpc(id, message),
            None => CommMsg::Data(message),
        };
        self.comm
            .outgoing_tx
            .send(comm_msg)
            .or_log_error("Connection Pane: Failed to send message to front end.");
    }
}

#[harp::register]
pub unsafe extern "C" fn ps_connection_opened(name: SEXP) -> Result<SEXP, anyhow::Error> {
    let main = RMain::get();
    let nm = RObject::view(name).to::<String>()?;

    let id = unwrap! (RConnection::start(nm, main.get_comm_manager_tx().clone()), Err(e) => {
        log::error!("Connection Pane: Failed to start connection: {e:?}");
        return Ok(R_NilValue);
    });

    Ok(RObject::from(id).into())
}

#[harp::register]
pub unsafe extern "C" fn ps_connection_closed(id: SEXP) -> Result<SEXP, anyhow::Error> {
    let main = RMain::get();
    let id_ = RObject::view(id).to::<String>()?;

    main.get_comm_manager_tx()
        .send(CommManagerEvent::Message(id_, CommMsg::Close))
        .or_log_error("Connection Pane: Failed to send comm_close to front end.");

    Ok(R_NilValue)
}
