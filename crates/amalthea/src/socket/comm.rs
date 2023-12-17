/*
 * comm.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use dyn_clone::DynClone;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

use crate::comm::base_comm::json_rpc_error;
use crate::comm::base_comm::JsonRpcErrorCode;
use crate::comm::comm_channel::CommMsg;

/**
 * A `CommSocket` is a relay between the back end and the front end of a comm.
 * It stores the comm's metadata and handles sending and receiving messages.
 *
 * The socket is a bi-directional channel between the front end and the back
 * end. The terms `incoming` and `outgoing` here refer to the direction of the
 * message flow; that is, `incoming` messages are messages that are received
 * from the front end, and `outgoing` messages are messages that are sent to the
 * front end.
 */
#[derive(Clone)]
pub struct CommSocket {
    /// The comm's unique identifier.
    pub comm_id: String,

    /// The comm's name. This is a freeform string, but it's typically a member
    /// of the Comm enum.
    pub comm_name: String,

    /// The identity of the comm's initiator. This is used to determine whether
    /// the comm is owned by the front end or the back end.
    pub initiator: CommInitiator,

    /// The channel receiving messages from the back end that are to be relayed
    /// to the front end (ultimately via IOPub). These messages are freeform
    /// JSON values.
    pub outgoing_rx: Receiver<CommMsg>,

    /// The other side of the channel receiving messages from the back end. This
    /// `Sender` is passed to the back end of the comm channel so that it can
    /// send messages to the front end.
    pub outgoing_tx: Sender<CommMsg>,

    /// The channel that will accept messages from the front end and relay them
    /// to the back end.
    pub incoming_tx: Sender<CommMsg>,

    /// The other side of the channel receiving messages from the front end
    pub incoming_rx: Receiver<CommMsg>,

    /// DOCME
    handlers: Option<Box<dyn CommHandling>>,
}

/**
 * Describes the identity of the comm's initiator. This is used to determine
 * whether the comm is owned by the front end or the back end.
 */
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CommInitiator {
    /// The comm was initiated by the front end (user interface).
    FrontEnd,

    /// The comm was initiated by the back end (kernel).
    BackEnd,
}

/**
 * A CommSocket is a relay between the back end and the front end of a comm
 * channel. It stores the comm's metadata and handles sending and receiving
 * messages.
 */
impl CommSocket {
    /**
     * Create a new CommSocket.
     *
     * - `initiator`: The identity of the comm's initiator. This is used to
     *   determine whether the comm is owned by the front end or the back end.
     * - `comm_id`: The comm's unique identifier.
     * - `comm_name`: The comm's name. This is a freeform string since comm
     *    names have no restrictions in the Jupyter protocol, but it's typically a
     *    member of the Comm enum.
     * - `handlers`: DOCME
     */
    pub fn new(
        initiator: CommInitiator,
        comm_id: String,
        comm_name: String,
        handlers: Option<Box<dyn CommHandling>>,
    ) -> Self {
        let (outgoing_tx, outgoing_rx) = crossbeam::channel::unbounded();
        let (incoming_tx, incoming_rx) = crossbeam::channel::unbounded();

        Self {
            comm_id,
            comm_name,
            initiator,
            outgoing_tx,
            outgoing_rx,
            incoming_tx,
            incoming_rx,
            handlers,
        }
    }

    pub fn handle_request(&self, message: CommMsg) -> anyhow::Result<bool> {
        let handlers = match self.handlers {
            Some(ref handlers) => handlers,
            None => {
                log::warn!("No message handlers defined for this comm");
                return Ok(false);
            },
        };

        let (id, data) = match message {
            CommMsg::Rpc(id, data) => (id, data),
            _ => return Ok(false),
        };

        let response = handlers.handle_request(id, data);
        self.outgoing_tx.send(response).unwrap();

        Ok(true)
    }
}

pub trait CommHandling: DynClone + Send + Sync {
    fn handle_request(&self, id: String, data: Value) -> CommMsg;
}

//  We need `Clone` on the `CommSocket` to send it across threads. We use
// the `dyn_clone` crate by dtolnay to help make our trait clonable in the
// dynamic case (e.g. `Box<dyn CommHandling>).
dyn_clone::clone_trait_object!(CommHandling);

/// DOCME
#[derive(Clone)]
pub struct CommHandlers<Evts, Reqs, Reps>
where
    Evts: Clone,
    Reqs: Clone,
    Reps: Clone,
{
    pub request_handler: fn(Reqs) -> anyhow::Result<Reps>,
    pub event_handler: fn(Evts) -> anyhow::Result<()>,
}

impl<Evts: Clone, Reqs: Clone, Reps: Clone> CommHandlers<Evts, Reqs, Reps> {
    pub fn new(
        event_handler: fn(Evts) -> anyhow::Result<()>,
        request_handler: fn(Reqs) -> anyhow::Result<Reps>,
    ) -> Self {
        Self {
            event_handler,
            request_handler,
        }
    }
}

impl<Evts, Reqs, Reps> CommHandling for CommHandlers<Evts, Reqs, Reps>
where
    Evts: Clone,
    Reqs: Clone + DeserializeOwned,
    Reps: Clone + Serialize,
{
    fn handle_request(&self, id: String, data: Value) -> CommMsg {
        let message = match serde_json::from_value::<Reqs>(data.clone()) {
            Ok(m) => m,
            Err(err) => {
                let json = json_rpc_error(
                    JsonRpcErrorCode::InvalidRequest,
                    format!("Invalid help request: {err:} (request: {data:})"),
                );
                return CommMsg::Rpc(id, json);
            },
        };

        let json = match (self.request_handler)(message) {
            Ok(reply) => match serde_json::to_value(reply) {
                Ok(value) => value,
                Err(err) => json_rpc_internal_error(err, data),
            },
            Err(err) => json_rpc_internal_error(err, data),
        };
        CommMsg::Rpc(id, json)
    }
}

fn json_rpc_internal_error<T>(err: T, data: Value) -> Value
where
    T: std::fmt::Display,
{
    json_rpc_error(
        JsonRpcErrorCode::InternalError,
        format!("Failed to process help request: {err} (request: {data:})"),
    )
}
