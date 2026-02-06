/*
 * comm.rs
 *
 * Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Receiver;
use crossbeam::channel::SendError;
use crossbeam::channel::Sender;
use serde::de::DeserializeOwned;
use serde::Serialize;
use stdext::result::ResultExt;

use crate::comm::base_comm::json_rpc_error;
use crate::comm::base_comm::JsonRpcErrorCode;
use crate::comm::comm_channel::CommMsg;
use crate::socket::iopub::IOPubMessage;

/// A sender for outgoing comm messages that routes through the IOPub channel.
///
/// This wrapper ensures comm messages go through the same channel as other
/// IOPub messages (like `ExecuteResult`), providing deterministic message
/// ordering when emitted from the same thread.
#[derive(Clone)]
pub struct CommOutgoingTx {
    comm_id: String,
    iopub_tx: Sender<IOPubMessage>,
}

impl CommOutgoingTx {
    /// Create a new `CommOutgoingTx` for a specific comm channel.
    pub fn new(comm_id: String, iopub_tx: Sender<IOPubMessage>) -> Self {
        Self { comm_id, iopub_tx }
    }

    /// Send an outgoing comm message through IOPub.
    pub fn send(&self, msg: CommMsg) -> Result<(), SendError<IOPubMessage>> {
        self.iopub_tx
            .send(IOPubMessage::CommOutgoing(self.comm_id.clone(), msg))
    }

    /// Get the underlying IOPub sender.
    ///
    /// This is useful when you need to create new `CommSocket`s that should
    /// route through the same IOPub channel.
    pub fn iopub_tx(&self) -> &Sender<IOPubMessage> {
        &self.iopub_tx
    }
}

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

    /// Sender for outgoing messages to the frontend. Routes through IOPub
    /// for deterministic message ordering.
    pub outgoing_tx: CommOutgoingTx,

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
     * - `iopub_tx`: The IOPub channel for sending messages to the frontend.
     */
    pub fn new(
        initiator: CommInitiator,
        comm_id: String,
        comm_name: String,
        iopub_tx: Sender<IOPubMessage>,
    ) -> Self {
        let (incoming_tx, incoming_rx) = crossbeam::channel::unbounded();
        let outgoing_tx = CommOutgoingTx::new(comm_id.clone(), iopub_tx);

        Self {
            comm_id,
            comm_name,
            initiator,
            outgoing_tx,
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
        let (id, parent_header, data) = match message {
            CommMsg::Rpc {
                id,
                parent_header,
                data,
            } => (id, parent_header, data),
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

        let response = CommMsg::Rpc {
            id,
            parent_header,
            data: json,
        };

        self.outgoing_tx.send(response).log_err();
        true
    }
}
