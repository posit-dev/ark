/*
 * interrupt_request.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// Represents request from the frontend to the kernel to get information
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InterruptRequest {}

impl MessageType for InterruptRequest {
    fn message_type() -> String {
        String::from("interrupt_request")
    }
}
