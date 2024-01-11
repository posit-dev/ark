/*
 * execute_result.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::wire::jupyter_message::MessageType;

/// Represents a request from the frontend to execute code
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExecuteResult {
    /// The data giving the result of the execution
    pub data: Value,

    /// A monotonically increasing execution counter
    pub execution_count: u32,

    /// Optional additional metadata
    pub metadata: Value,
}

impl MessageType for ExecuteResult {
    fn message_type() -> String {
        String::from("execute_result")
    }
}
