/*
 * display_data.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::wire::jupyter_message::MessageType;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DisplayData {
    /// The data giving the MIME key/value pairs to display
    pub data: Value,

    /// Optional additional metadata
    pub metadata: Value,

    /// Optional transient data
    pub transient: Value,
}

impl MessageType for DisplayData {
    fn message_type() -> String {
        String::from("display_data")
    }
}
