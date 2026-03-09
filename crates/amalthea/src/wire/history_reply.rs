/*
 * history_reply.rs
 *
 * Copyright (C) 2026 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;
use crate::wire::jupyter_message::Status;

/// A single history entry, either `(session, line_number, input)` when
/// `output` was false, or `(session, line_number, (input, output))` when true.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum HistoryEntry {
    Input(i64, i64, String),
    InputOutput(i64, i64, (String, String)),
}

/// Represents a reply to a history_request.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HistoryReply {
    pub status: Status,
    pub history: Vec<HistoryEntry>,
}

impl MessageType for HistoryReply {
    fn message_type() -> String {
        String::from("history_reply")
    }
}
