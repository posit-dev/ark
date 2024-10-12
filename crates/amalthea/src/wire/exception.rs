/*
 * exception.rs
 *
 * Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

/// Represents a runtime exception on a ROUTER/DEALER socket
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Exception {
    /// The name of the exception
    pub ename: String,

    /// The value/description of the exception
    pub evalue: String,

    /// List of traceback frames, as strings
    pub traceback: Vec<String>,
}

impl Exception {
    pub fn internal_error(evalue: String) -> Self {
        Self {
            ename: String::from("InternalError"),
            evalue,
            traceback: vec![],
        }
    }
}
