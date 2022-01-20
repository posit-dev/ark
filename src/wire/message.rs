/*
 * message.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::header::JupyterHeader;
use serde::Serialize;

/// This delimiter separates the ZeroMQ socket identities (IDS) from the message
/// body payload (MSG).
const MSG_DELIM: &[u8] = b"<IDS|MSG>";

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

impl JupyterMessage<T> {
    /// Parse a Jupyter message from an array of buffers (from a ZeroMQ message)
    pub fn from_buffers(
        mut bufs: Vec<Vec<u8>>,
    ) -> Result<JupyterMessage, Box<dyn std::error::Error>> {
        let iter = bufs.iter();
        if let Some(pos) = iter.position(|buf| &buf[..] == MSG_DELIM) {
            return from_msg_bufs(bufs[pos+1..]);
        }
        // TODO: need real error
        return Err()
    }

    fn from_msg_bufs(mut bufs: Vec<Vec<u8>>) -> Result<JupyterMessage, Box<dyn std::error::Error>> {
        
    }

    }
}
