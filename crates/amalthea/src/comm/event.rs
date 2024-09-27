/*
 * comm_event.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Sender;
use serde_json::Value;

use crate::comm::comm_channel::CommMsg;
use crate::socket::comm::CommSocket;
use crate::wire::header::JupyterHeader;

/**
 * Enumeration of events that can be received by the comm manager.
 */
pub enum CommManagerEvent {
    /// A new Comm was opened
    Opened(CommSocket, Value),

    /// A message was received on a Comm; the first value is the comm ID, and the
    /// second value is the message.
    Message(String, CommMsg),

    /// An RPC was received from the frontend
    PendingRpc(JupyterHeader),

    /// A Comm was closed
    Closed(String),

    /// A comm manager request
    Request(CommManagerRequest),
}

/**
 * Enumeration of requests that can be received by the comm manager.
 */
pub enum CommManagerRequest {
    /// Open comm information
    Info(Sender<CommManagerInfoReply>),
}

pub struct CommManagerInfoReply {
    pub comms: Vec<CommInfo>,
}

pub struct CommInfo {
    pub id: String,
    pub name: String,
}
