/*
 * is_complete_reply.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::jupyter_message::MessageType;
use serde::{Deserialize, Serialize};

/// Represents a reply to an is_complete_request.
#[derive(Debug, Serialize, Deserialize)]
pub struct IsCompleteReply {
    /// The status of the code: one of Complete, Incomplete, Invalid, or Unknown
    /// (TODO: make this an enum)
    pub status: String,

    /// Characters to use for indenting the next line (if incomplete)
    pub indent: String,
}

impl MessageType for IsCompleteRequest {
    fn message_type() -> String {
        String::from("is_complete_reply")
    }
}
