/*
 * debug_request.rs
 *
 * Copyright (C) 2026 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// Represents a Jupyter Debug Protocol request.
///
/// The content is an opaque DAP (Debug Adapter Protocol) request message,
/// forwarded as-is between the frontend and the kernel's debugger.
///
/// https://jupyter-client.readthedocs.io/en/latest/messaging.html#debug-request
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DebugRequest {
    #[serde(flatten)]
    pub content: serde_json::Value,
}

impl MessageType for DebugRequest {
    fn message_type() -> String {
        String::from("debug_request")
    }
}
