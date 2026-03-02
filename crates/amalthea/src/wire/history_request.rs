/*
 * history_request.rs
 *
 * Copyright (C) 2026 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// Represents a request from the frontend for execution history.
///
/// The protocol defines three access types (`range`, `tail`, `search`)
/// with different fields for each.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "hist_access_type")]
pub enum HistoryRequest {
    #[serde(rename = "range")]
    Range {
        #[serde(default)]
        output: bool,
        #[serde(default)]
        raw: bool,
        #[serde(default)]
        session: i64,
        #[serde(default)]
        start: i64,
        #[serde(default)]
        stop: i64,
    },

    #[serde(rename = "tail")]
    Tail {
        #[serde(default)]
        output: bool,
        #[serde(default)]
        raw: bool,
        #[serde(default)]
        n: i64,
    },

    #[serde(rename = "search")]
    Search {
        #[serde(default)]
        output: bool,
        #[serde(default)]
        raw: bool,
        #[serde(default)]
        n: i64,
        #[serde(default)]
        pattern: String,
        #[serde(default)]
        unique: bool,
    },
}

impl MessageType for HistoryRequest {
    fn message_type() -> String {
        String::from("history_request")
    }
}
