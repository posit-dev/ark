//
// connection.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::connections_comm::ConnectionsBackendReply;
use amalthea::comm::connections_comm::ConnectionsBackendRequest;
use amalthea::comm::connections_comm::ContainsDataParams;
use amalthea::comm::connections_comm::FieldSchema;
use amalthea::comm::connections_comm::GetIconParams;
use amalthea::comm::connections_comm::ListFieldsParams;
use amalthea::comm::connections_comm::ListObjectsParams;
use amalthea::comm::connections_comm::ObjectSchema;
use amalthea::comm::connections_comm::PreviewObjectParams;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use crossbeam::channel::Sender;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::utils::r_is_null;
use libr::R_NilValue;
use libr::SEXP;
use serde::Deserialize;
use serde::Serialize;
use stdext::result::ResultOrLog;
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::interface::RMain;
use crate::r_task;

#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectionTable {
    name: String,
    kind: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ConnectionTableField {
    name: String,
    dtype: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum ConnectionResponse {
    TablesResponse {
        name: String,
        tables: Vec<ConnectionTable>,
    },
    FieldsResponse {
        name: String,
        fields: Vec<ConnectionTableField>,
    },
    PreviewResponse,
    IconResponse {
        icon: Option<String>,
    },
    ContainsDataResponse {
        contains_data: bool,
    },
    ConnectionsBackendReply,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum ConnectionRequest {
    // The UI is asking for the list of tables in the connection.
    TablesRequest { path: Vec<ConnectionTable> },
    // The UI is asking for the list of fields in a table.
    FieldsRequest { path: Vec<ConnectionTable> },
    // The UI asks for a DataViewer preview of the table.
    PreviewTable { path: Vec<ConnectionTable> },
    // The UI asks for an icon for a given element
    IconRequest { path: Vec<ConnectionTable> },
    // The UI asks if the object contains data
    ContainsDataRequest { path: Vec<ConnectionTable> },
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
            unwrap!(connection.handle_messages(), Err(err) => {
                log::error!("Connection Pane: Error while handling messages: {err:?}");
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

    fn handle_rpc(
        &self,
        message: ConnectionsBackendRequest,
    ) -> Result<ConnectionsBackendReply, anyhow::Error> {
        match message {
            ConnectionsBackendRequest::ListObjects(ListObjectsParams { path }) => {
                let tables = r_task(|| -> Result<_, anyhow::Error> {
                    unsafe {
                        let mut call = RFunction::from(".ps.connection_list_objects");
                        call.add(RObject::from(self.comm.comm_id.clone()));
                        for obj in path {
                            call.param(obj.kind.as_str(), obj.name);
                        }
                        // returns a data.frame with columns name and type
                        let tables = call.call()?;

                        let names = RFunction::from("[[")
                            .add(tables.clone())
                            .add(RObject::from("name"))
                            .call()?;

                        let types = RFunction::from("[[")
                            .add(tables)
                            .add(RObject::from("type"))
                            .call()?;

                        let resulting = RObject::to::<Vec<String>>(names)?
                            .iter()
                            .zip(RObject::to::<Vec<String>>(types)?.iter())
                            .map(|(name, kind)| ObjectSchema {
                                name: name.clone(),
                                kind: kind.clone(),
                            })
                            .collect::<Vec<_>>();

                        Ok(resulting)
                    }
                })?;

                Ok(ConnectionsBackendReply::ListObjectsReply(tables))
            },
            ConnectionsBackendRequest::ListFields(ListFieldsParams { path }) => {
                let fields = r_task(|| -> Result<_, anyhow::Error> {
                    unsafe {
                        let mut call = RFunction::from(".ps.connection_list_fields");
                        call.add(RObject::from(self.comm.comm_id.clone()));
                        for obj in path {
                            call.param(obj.kind.as_str(), obj.name);
                        }
                        let fields = call.call()?;

                        // for now we only need the name column
                        let names = RFunction::from("[[")
                            .add(fields.clone())
                            .add(RObject::from("name"))
                            .call()?;

                        let dtypes = RFunction::from("[[")
                            .add(fields)
                            .add(RObject::from("type"))
                            .call()?;

                        let resulting = RObject::to::<Vec<String>>(names)?
                            .iter()
                            .zip(RObject::to::<Vec<String>>(dtypes)?.iter())
                            .map(|(name, dtype)| FieldSchema {
                                name: name.clone(),
                                dtype: dtype.clone(),
                            })
                            .collect::<Vec<_>>();

                        Ok(resulting)
                    }
                })?;

                Ok(ConnectionsBackendReply::ListFieldsReply(fields))
            },
            ConnectionsBackendRequest::PreviewObject(PreviewObjectParams { path }) => {
                // Calls back into R to get the preview data.
                r_task(|| -> Result<(), anyhow::Error> {
                    let mut call = RFunction::from(".ps.connection_preview_object");
                    call.add(RObject::from(self.comm.comm_id.clone()));
                    for obj in path {
                        call.param(obj.kind.as_str(), obj.name);
                    }
                    call.call()?;
                    Ok(())
                })?;
                Ok(ConnectionsBackendReply::PreviewObjectReply())
            },
            ConnectionsBackendRequest::GetIcon(GetIconParams { path }) => {
                // Calls back into R to get the icon.
                let icon_path = r_task(|| -> Result<_, anyhow::Error> {
                    unsafe {
                        let mut call = RFunction::from(".ps.connection_icon");
                        call.add(RObject::from(self.comm.comm_id.clone()));
                        for obj in path {
                            call.param(obj.kind.as_str(), obj.name);
                        }

                        let icon = call.call()?;

                        if r_is_null(*icon) {
                            // we'd rather use the option type but couldn't find a way to autogenerate RPC optionals
                            Ok("".to_string())
                        } else {
                            Ok(RObject::to::<String>(icon)?)
                        }
                    }
                })?;
                Ok(ConnectionsBackendReply::GetIconReply(icon_path))
            },
            ConnectionsBackendRequest::ContainsData(ContainsDataParams { path }) => {
                // Calls back into R to check if the object contains data.
                let contains_data = r_task(|| -> Result<_, anyhow::Error> {
                    unsafe {
                        let mut contains_data_call: RFunction =
                            RFunction::from(".ps.connection_contains_data");
                        contains_data_call.add(RObject::from(self.comm.comm_id.clone()));
                        for obj in path {
                            contains_data_call.param(obj.kind.as_str(), obj.name);
                        }
                        let contains_data = contains_data_call.call()?;
                        Ok(RObject::to::<bool>(contains_data)?)
                    }
                })?;
                Ok(ConnectionsBackendReply::ContainsDataReply(contains_data))
            },
        }
    }

    fn disconnect(&self) -> std::result::Result<(), anyhow::Error> {
        // Execute database side disconnect method.
        r_task(|| -> Result<(), anyhow::Error> {
            let mut call = RFunction::from(".ps.connection_close");
            call.add(RObject::from(self.comm.comm_id.clone()));
            call.call()?;
            Ok(())
        })
    }

    fn handle_messages(&self) -> Result<(), anyhow::Error> {
        loop {
            let msg = unwrap!(self.comm.incoming_rx.recv(), Err(err) => {
                log::error!("Connection Pane: Error while receiving message from frontend: {err:?}");
                break;
            });

            log::trace!("Connection Pane: Received message from front end: {msg:?}");

            if let CommMsg::Close = msg {
                log::trace!("Connection Pane: Received a close message.");
                self.disconnect()?;
                break;
            }

            self.comm.handle_request(msg, |req| self.handle_rpc(req));
        }

        // before finalizing the thread we make sure to send a close message to the front end
        if let Err(err) = self.comm.outgoing_tx.send(CommMsg::Close) {
            log::error!("Connection Pane: Error while sending comm_close to front end: {err:?}");
        }

        Ok(())
    }
}

#[harp::register]
pub unsafe extern "C" fn ps_connection_opened(name: SEXP) -> Result<SEXP, anyhow::Error> {
    let main = RMain::get();
    let nm = RObject::view(name).to::<String>()?;

    let id = unwrap! (RConnection::start(nm, main.get_comm_manager_tx().clone()), Err(err) => {
        log::error!("Connection Pane: Failed to start connection: {err:?}");
        return Err(err);
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
