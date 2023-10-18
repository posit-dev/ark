/*
 * shutdown_reply.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use super::jupyter_message::Status;
use crate::wire::jupyter_message::MessageType;

/// Represents reply from the kernel to a shutdown request.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShutdownReply {
    /// Error flag
    pub status: Status,

    /// False if final shutdown; true if shutdown precedes a restart
    pub restart: bool,
}

impl MessageType for ShutdownReply {
    fn message_type() -> String {
        String::from("shutdown_reply")
    }
}
