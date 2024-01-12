/*
 * shutdown_request.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// Represents request from the frontend to the kernel to get information
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShutdownRequest {
    /// False if final shutdown; true if shutdown precedes a restart
    pub restart: bool,
}

impl MessageType for ShutdownRequest {
    fn message_type() -> String {
        String::from("shutdown_request")
    }
}
