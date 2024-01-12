/*
 * input_reply.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// Represents a reply from the frontend to the kernel delivering the response
/// to an `input_request`
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InputReply {
    /// The value the user entered
    pub value: String,
}

impl MessageType for InputReply {
    fn message_type() -> String {
        String::from("input_reply")
    }
}
