/*
 * execute_error.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::exception::Exception;
use crate::wire::jupyter_message::MessageType;
use serde::{Deserialize, Serialize};

/// Represents an exception that occurred while executing code
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
