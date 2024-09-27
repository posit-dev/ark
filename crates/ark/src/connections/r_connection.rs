//
// connection.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::connections_comm::ConnectionsBackendReply;
use amalthea::comm::connections_comm::ConnectionsBackendRequest;
use amalthea::comm::connections_comm::ConnectionsFrontendEvent;
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
use stdext::spawn;
use stdext::unwrap;
use uuid::Uuid;

use crate::interface::RMain;
use crate::r_task;

#[derive(Deserialize, Serialize, Clone)]
pub struct Metadata {
    pub name: String,
    pub language_id: String,
    pub host: Option<String>,
    pub r#type: Option<String>, // r#type is used to avoid conflict with the type keyword
    pub code: Option<String>,
}

pub struct RConnection {
    metadata: Metadata,
    comm: CommSocket,
    comm_manager_tx: Sender<CommManagerEvent>,
}

impl RConnection {
    pub fn start(
        metadata: Metadata,
        comm_manager_tx: Sender<CommManagerEvent>,
        comm_id: String,
    ) -> Result<String, anyhow::Error> {
        let comm = CommSocket::new(
            CommInitiator::BackEnd,
            comm_id.clone(),
            String::from("positron.connection"),
        );

        let connection = Self {
            metadata,
            comm,
            comm_manager_tx,
        };

        log::info!("Connection Pane: Channel created id:{comm_id}");
        connection.open_and_register_comm()?;

        spawn!(format!("ark-connection-{}", comm_id), move || {
            unwrap!(connection.handle_messages(), Err(err) => {
                log::error!("Connection Pane: Error while handling messages: {err:?}");
            });
        });

        Ok(comm_id)
    }

    fn open_and_register_comm(&self) -> Result<(), anyhow::Error> {
        let comm_open_json = serde_json::to_value(self.metadata.clone())?;

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

    fn disconnect(&self) -> std::result::Result<bool, anyhow::Error> {
        // Execute database side disconnect method.
        r_task(|| -> Result<bool, anyhow::Error> {
            unsafe {
                let mut call = RFunction::from(".ps.connection_close");
                call.add(RObject::from(self.comm.comm_id.clone()));
                let closed = call.call()?;
                Ok(RObject::to::<bool>(closed)?)
            }
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
                let disconnected = self.disconnect()?;
                if !disconnected {
                    self.comm.outgoing_tx.send(CommMsg::Close).unwrap();
                }
                break;
            }

            // Forward data msgs to the frontend
            if let CommMsg::Data(_) = msg {
                self.comm.outgoing_tx.send(msg)?;
                continue;
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
pub unsafe extern "C" fn ps_connection_opened(
    name: SEXP,
    host: SEXP,
    r#type: SEXP,
    code: SEXP,
) -> Result<SEXP, anyhow::Error> {
    let id = Uuid::new_v4().to_string();
    let id_r: RObject = id.clone().into();

    if harp::test::IS_TESTING {
        // If RMain is not initialized, we are probably in testing mode, so we just don't start the connection
        // and let the testing code manually do it
        log::warn!("Connection Pane: RMain is not initialized. Connection will not be started.");
        return Ok(id_r.sexp);
    }

    let main = RMain::get();

    let metadata = Metadata {
        name: RObject::view(name).to::<String>()?,
        language_id: String::from("r"),
        host: RObject::view(host).to::<Option<String>>().unwrap_or(None),
        r#type: RObject::view(r#type).to::<Option<String>>().unwrap_or(None),
        code: RObject::view(code).to::<Option<String>>().unwrap_or(None),
    };

    if let Err(err) = RConnection::start(metadata, main.get_comm_manager_tx().clone(), id) {
        log::error!("Connection Pane: Failed to start connection: {err:?}");
        return Err(err);
    }

    return Ok(id_r.sexp);
}

#[harp::register]
pub unsafe extern "C" fn ps_connection_closed(id: SEXP) -> Result<SEXP, anyhow::Error> {
    let main = RMain::get();
    let id_ = RObject::view(id).to::<String>()?;

    main.get_comm_manager_tx()
        .send(CommManagerEvent::Message(id_, CommMsg::Close))?;

    Ok(R_NilValue)
}

#[harp::register]
pub unsafe extern "C" fn ps_connection_updated(id: SEXP) -> Result<SEXP, anyhow::Error> {
    let main = RMain::get();
    let comm_id: String = RObject::view(id).to::<String>()?;

    let event = ConnectionsFrontendEvent::Update;

    main.get_comm_manager_tx().send(CommManagerEvent::Message(
        comm_id,
        CommMsg::Data(serde_json::to_value(event)?),
    ))?;

    Ok(R_NilValue)
}
