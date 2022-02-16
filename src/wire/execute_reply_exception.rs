/*
 * execute_reply_exception.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::exception::Exception;
use crate::wire::jupyter_message::MessageType;
use serde::{Deserialize, Serialize};

/// Represents completion possibilities for a code fragment supplied by the front end.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExecuteReplyException {
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
