/*
 * comm_info_request.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;
use serde_json::Map;
use serde_json::Value;

use crate::wire::jupyter_message::MessageType;
use crate::wire::jupyter_message::Status;

/// Represents a reply from the kernel listing open comms
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommInfoReply {
    /// The status of the request (usually "ok")
    pub status: Status,

    /// Dictionary of comms, indexed by UUID
    pub comms: Map<String, Value>,
}

/// Represents comm info for a single target
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommInfoTargetName {
    pub target_name: String,
}

impl MessageType for CommInfoReply {
    fn message_type() -> String {
        String::from("comm_info_reply")
    }
}
