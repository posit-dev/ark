/*
 * comm_info_request.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// Represents a request from the frontend to show open comms
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommInfoRequest {
    pub target_name: String,
}

impl MessageType for CommInfoRequest {
    fn message_type() -> String {
        String::from("comm_info_request")
    }
}
