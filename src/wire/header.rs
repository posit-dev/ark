/*
 * header.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use serde::{Deserialize, Serialize};

/// Represents the header of a Jupyter message
#[derive(Serialize, Deserialize)]
pub struct JupyterHeader {
    /// The message identifier; must be unique per message
    pub msg_id: String,

    /// Session ID; must be unique per session
    pub session_id: String,

    /// Username; must be unique per user
    pub username: String,

    /// Date/time when message was created (ISO 8601)
    pub date: String,

    /// Message type
    pub msg_type: String,

    /// Message protocol version
    pub version: String,
}
