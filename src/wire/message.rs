/*
 * message.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::header::JupyterHeader;
use serde::Serialize;

/// Represents a Jupyter message
#[derive(Serialize)]
pub struct JupyterMessage<T> {
    /// The header for this message
    pub header: JupyterHeader,

    /// The header of the message from which this message originated
    pub parent_header: JupyterHeader,

    /// Additional metadata, if any
    pub metadata: (),

    /// The body (payload) of the message
    pub content: T,

    /// Additional binary data
    pub buffers: (),
}
