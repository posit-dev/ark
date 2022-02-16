/*
 * exception.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::jupyter_message::Status;
use serde::{Deserialize, Serialize};

/// Represents a runtime exception on a ROUTER/DEALER socket
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Exception {
    /// The status; always Error
    pub status: Status,

    /// The name of the exception
    pub ename: String,

    /// The value/description of the exception
    pub evalue: String,

    /// List of traceback frames, as strings
    pub traceback: Vec<String>,
}
