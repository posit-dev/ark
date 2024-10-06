/*
 * execute_error.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::exception::Exception;
use crate::wire::jupyter_message::MessageType;

/// Represents an exception that occurred while executing code.
/// This is sent to IOPub. Not to be confused with `ExecuteReplyException`
/// which is a special case of a message of type `"execute_reply"` sent to Shell
/// in response to an `"execute_request"`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExecuteError {
    /// The exception that occurred during execution
    #[serde(flatten)]
    pub exception: Exception,
}

impl MessageType for ExecuteError {
    fn message_type() -> String {
        String::from("error")
    }
}
