/*
 * comm.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::comm::base_comm::json_rpc_error;
use crate::comm::base_comm::JsonRpcErrorCode;
use crate::comm::comm_channel::CommMsg;

/**
 * A `CommSocket` is a relay between the back end and the frontend of a comm.
 * It stores the comm's metadata and handles sending and receiving messages.
 *
 * The socket is a bi-directional channel between the frontend and the back
 * end. The terms `incoming` and `outgoing` here refer to the direction of the
 * message flow; that is, `incoming` messages are messages that are received
 * from the frontend, and `outgoing` messages are messages that are sent to the
 * frontend.
 */
#[derive(Clone)]
pub struct CommSocket {
    /// The comm's unique identifier.
    pub comm_id: String,

    /// The comm's name. This is a freeform string, but it's typically a member
    /// of the Comm enum.
    pub comm_name: String,

    /// The identity of the comm's initiator. This is used to determine whether
    /// the comm is owned by the frontend or the back end.
    pub initiator: CommInitiator,

    /// The channel receiving messages from the back end that are to be relayed
    /// to the frontend (ultimately via IOPub). These messages are freeform
    /// JSON values.
    pub outgoing_rx: Receiver<CommMsg>,

    /// The other side of the channel receiving messages from the back end. This
    /// `Sender` is passed to the back end of the comm channel so that it can
    /// send messages to the frontend.
    pub outgoing_tx: Sender<CommMsg>,

    /// The channel that will accept messages from the frontend and relay them
    /// to the back end.
    pub incoming_tx: Sender<CommMsg>,

    /// The other side of the channel receiving messages from the frontend
    pub incoming_rx: Receiver<CommMsg>,
}

/**
 * Describes the identity of the comm's initiator. This is used to determine
 * whether the comm is owned by the frontend or the back end.
 */
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CommInitiator {
    /// The comm was initiated by the frontend (user interface).
    FrontEnd,

    /// The comm was initiated by the back end (kernel).
    BackEnd,
}

/**
 * A CommSocket is a relay between the back end and the frontend of a comm
 * channel. It stores the comm's metadata and handles sending and receiving
 * messages.
 */
impl CommSocket {
    /**
     * Create a new CommSocket.
     *
     * - `initiator`: The identity of the comm's initiator. This is used to
     *   determine whether the comm is owned by the frontend or the back end.
     * - `comm_id`: The comm's unique identifier.
     * - `comm_name`: The comm's name. This is a freeform string since comm
     *    names have no restrictions in the Jupyter protocol, but it's typically a
     *    member of the Comm enum.
     */
    pub fn new(initiator: CommInitiator, comm_id: String, comm_name: String) -> Self {
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
        }
    }

    /**
     * Handle `CommMsg::Rpc`.
     *
     * - `message`: A message received by the comm.
     * - `request_handler`: The comm's handler for requests.
     *
     * Returns `false` if `message` is not an RPC. Otherwise returns `true`.
     * Requests that could not be handled cause an RPC error response.
     */
    pub fn handle_request<Reqs, Reps>(
        &self,
        message: CommMsg,
        request_handler: impl FnOnce(Reqs) -> anyhow::Result<Reps>,
    ) -> bool
    where
        Reqs: DeserializeOwned + std::fmt::Debug,
        Reps: Serialize,
    {
        let (id, data) = match message {
            CommMsg::Rpc(id, data) => (id, data),
            _ => return false,
        };

        let json = match serde_json::from_value::<Reqs>(data.clone()) {
            Ok(m) => {
                let _span =
                    tracing::trace_span!("comm handler", name = ?self.comm_name, request = ?m)
                        .entered();
                match request_handler(m) {
                    Ok(reply) => match serde_json::to_value(reply) {
                        Ok(value) => value,
                        Err(err) => {
                            let message = format!(
                                        "Failed to serialise reply for {} request: {err} (request: {data:})",
                                        self.comm_name
                                    );
                            log::trace!("{message}");
                            json_rpc_error(JsonRpcErrorCode::InternalError, message)
                        },
                    },
                    Err(err) => {
                        let message = format!(
                            "Failed to process {} request: {err} (request: {data:})",
                            self.comm_name
                        );
                        log::trace!("{message}");
                        json_rpc_error(JsonRpcErrorCode::InternalError, message)
                    },
                }
            },
            Err(err) => {
                let message = format!(
                    "No handler for {} request (method not found): {err:} (request: {data:})",
                    self.comm_name
                );
                log::trace!("{message}");
                json_rpc_error(JsonRpcErrorCode::MethodNotFound, message)
            },
        };

        let response = CommMsg::Rpc(id, json);

        self.outgoing_tx.send(response).unwrap();
        true
    }
}
