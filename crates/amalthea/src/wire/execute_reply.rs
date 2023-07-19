/*
 * execute_reply.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::wire::jupyter_message::MessageType;
use crate::wire::jupyter_message::Status;

/// Represents a reply from an execute_request message
#[serde_with::skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExecuteReply {
    /// The status of the request
    pub status: Status,

    /// Monotonically increasing execution counter
    pub execution_count: u32,

    /// Results for user expressions
    pub user_expressions: Value,

    /// Extra Posit fields. Currently only used for prompt strings.
    pub positron: Option<ExecuteReplyPositron>,
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExecuteReplyPositron {
    pub input_prompt: Option<String>,
    pub continuation_prompt: Option<String>,
    pub is_input_request: Option<bool>,
}

impl MessageType for ExecuteReply {
    fn message_type() -> String {
        String::from("execute_reply")
    }
}
