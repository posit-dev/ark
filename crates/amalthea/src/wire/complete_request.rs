/*
 * complete_request.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// Represents a request from the frontend to show possibilities for completing
/// a code fragment.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CompleteRequest {
    /// The code fragment to complete.
    pub code: String,
    /// The position of the cursor in the incomplete code.
    pub cursor_pos: u32,
}

impl MessageType for CompleteRequest {
    fn message_type() -> String {
        String::from("complete_request")
    }
}
