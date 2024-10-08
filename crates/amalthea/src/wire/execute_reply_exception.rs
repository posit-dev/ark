/*
 * execute_reply_exception.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::exception::Exception;
use crate::wire::jupyter_message::MessageType;
use crate::wire::jupyter_message::Status;

/// Represents an exception that occurred while executing code
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExecuteReplyException {
    /// The status; always Error
    pub status: Status,

    /// The execution counter
    pub execution_count: u32,

    /// The exception that occurred during execution
    #[serde(flatten)]
    pub exception: Exception,
}

impl MessageType for ExecuteReplyException {
    fn message_type() -> String {
        String::from("execute_reply")
    }
}
