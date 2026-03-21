/*
 * debug_reply.rs
 *
 * Copyright (C) 2026 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// Represents a reply to a `debug_request` message on the control channel.
///
/// The content is an opaque DAP (Debug Adapter Protocol) response, passed
/// through as-is between the frontend and the kernel's debugger.
///
/// https://jupyter-client.readthedocs.io/en/latest/messaging.html#debug-request
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DebugReply {
    #[serde(flatten)]
    pub content: serde_json::Value,
}

impl MessageType for DebugReply {
    fn message_type() -> String {
        String::from("debug_reply")
    }
}
