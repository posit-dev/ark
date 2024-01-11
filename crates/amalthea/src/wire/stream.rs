/*
 * stream.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// Represents a message from the frontend to indicate stream output
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StreamOutput {
    /// The name of the stream for which output is being emitted
    pub name: Stream,

    /// The output emitted on the stream
    pub text: String,
}

impl MessageType for StreamOutput {
    fn message_type() -> String {
        String::from("stream")
    }
}

#[derive(Debug, Serialize, Deserialize, Copy, Clone, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    /// Standard output
    Stdout,

    /// Standard error
    Stderr,
}
