/*
 * complete_request.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::jupyter_message::MessageType;
use crate::wire::jupyter_message::SocketType;
use serde::{Deserialize, Serialize};

/// Represents a request from the front end to show possibilities for completing
/// a code fragment.
#[derive(Debug, Serialize, Deserialize)]
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
    fn socket_type() -> SocketType {
        SocketType::Shell
    }
}
