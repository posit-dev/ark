/*
 * execute_result.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::jupyter_message::MessageType;
use crate::wire::jupyter_message::SocketType;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Represents a request from the front end to execute code
#[derive(Debug, Serialize, Deserialize)]
pub struct ExecuteResult {
    /// The data giving the result of the execution
    data: Value,

    /// A monotonically increasing execution counter
    execution_count: u32,

    /// Optional additional metadata
    metadata: Value,
}

impl MessageType for ExecuteResult {
    fn message_type() -> String {
        String::from("execute_result")
    }
    fn socket_type() -> SocketType {
        SocketType::IOPub
    }
}
