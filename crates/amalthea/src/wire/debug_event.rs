/*
 * debug_event.rs
 *
 * Copyright (C) 2026 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// Represents a debug event published on the IOPub channel.
///
/// The content is an opaque DAP (Debug Adapter Protocol) event, forwarded
/// as-is between the kernel's debugger and the frontend.
///
/// https://jupyter-client.readthedocs.io/en/latest/messaging.html#additions-to-the-dap
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DebugEvent {
    #[serde(flatten)]
    pub content: serde_json::Value,
}

impl MessageType for DebugEvent {
    fn message_type() -> String {
        String::from("debug_event")
    }
}
