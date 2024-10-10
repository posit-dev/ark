/*
 * handshake_reply.rs
 *
 * Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;
use crate::wire::jupyter_message::Status;

/// Represents a reply to a handshake_request
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HandshakeReply {
    /// The execution status ("ok" or "error")
    pub status: Status,
}

impl MessageType for HandshakeReply {
    fn message_type() -> String {
        String::from("handshake_reply")
    }
}
