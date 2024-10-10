/*
 * handshake_request.rs
 *
 * Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// Represents a handshake_request
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HandshakeRequest {
    /// ZeroMQ port: Control channel (kernel interrupts)
    pub control_port: u16,

    /// ZeroMQ port: Shell channel (execution, completion)
    pub shell_port: u16,

    /// ZeroMQ port: Standard input channel (prompts)
    pub stdin_port: u16,

    /// ZeroMQ port: IOPub channel (broadcasts input/output)
    pub iopub_port: u16,

    /// ZeroMQ port: Heartbeat messages (echo)
    pub hb_port: u16,
}

impl MessageType for HandshakeRequest {
    fn message_type() -> String {
        String::from("handshake_request")
    }
}
