/*
 * update_display_data.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::wire::jupyter_message::MessageType;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UpdateDisplayData {
    /// The data giving the MIME key/value pairs to display
    pub data: Value,

    /// Optional additional metadata
    pub metadata: Value,

    /// Transient data
    /// Must contain a `display_id` field linked to one in a
    /// corresponding `DisplayData` message.
    pub transient: Value,
}

impl MessageType for UpdateDisplayData {
    fn message_type() -> String {
        String::from("update_display_data")
    }
}
