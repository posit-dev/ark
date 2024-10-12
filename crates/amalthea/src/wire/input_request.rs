/*
 * input_request.rs
 *
 * Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Sender;
use serde::Deserialize;
use serde::Serialize;

use super::originator::Originator;
use crate::comm::base_comm::JsonRpcReply;
use crate::comm::ui_comm::UiFrontendRequest;
use crate::wire::jupyter_message::MessageType;

/// Represents a request from the kernel to the frontend to prompt the user for
/// input
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InputRequest {
    /// The prompt to display to the user
    pub prompt: String,

    /// Whether the string being requested is a password (and should therefore
    /// be obscured)
    pub password: bool,
}

/// An input request originating from a Shell handler
pub struct ShellInputRequest {
    /// The identity of the Shell that sent the request
    pub originator: Originator,

    /// The input request itself
    pub request: InputRequest,
}

impl MessageType for InputRequest {
    fn message_type() -> String {
        String::from("input_request")
    }
}

/// A Comm request for StdIn
#[derive(Debug, Clone)]
pub struct UiCommFrontendRequest {
    /// The identity of the currently active `execute_request` that caused this
    /// comm request
    pub originator: Originator,

    /// The reply channel for the request
    pub reply_tx: Sender<StdInRpcReply>,

    /// The actual comm request
    pub request: UiFrontendRequest,
}

#[derive(Debug, Clone)]
pub enum StdInRpcReply {
    Reply(JsonRpcReply),
    Interrupt,
}
