/*
 * inspect_reply.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::jupyter_message::MessageType;
use crate::wire::jupyter_message::Status;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Represents a reply from the kernel giving code inspection results
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InspectReply {
    /// The status of the request (usually Ok)
    status: Status,

    /// True if an object was found
    found: bool,

    /// MIME bundle giving information about the object
    data: Value,

    /// Additional metadata
    metadata: Value,
}

impl MessageType for InspectReply {
    fn message_type() -> String {
        String::from("inspect_reply")
    }
}
