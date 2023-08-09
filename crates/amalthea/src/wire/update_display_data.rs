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
    pub transient: TransientValue,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TransientValue {
    /// An identifier to link an `UpdateDisplayData` message with its
    /// corresponding `DisplayData` message.
    pub display_id: String,

    /// Additional optional transient data. Always flattened to
    /// the same level as the required `display_id`.
    #[serde(flatten)]
    pub data: Option<Value>,
}

impl MessageType for UpdateDisplayData {
    fn message_type() -> String {
        String::from("update_display_data")
    }
}
