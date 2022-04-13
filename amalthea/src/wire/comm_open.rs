/*
 * comm_open.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::jupyter_message::MessageType;
use serde::{Deserialize, Serialize};

/// Represents a request to open a custom comm
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommOpen {
    comm_id: String,
    target_name: String,
    data: serde_json::Value,
}

impl MessageType for CommOpen {
    fn message_type() -> String {
        String::from("comm_open")
    }
}
