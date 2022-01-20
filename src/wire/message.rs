/*
 * message.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::header::JupyterHeader;
use hmac::Hmac;
use serde::Serialize;
use std::fmt;

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

#[derive(Debug)]
pub enum MessageError {
    MissingDelimiter,
}

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MessageError::MissingDelimiter => {
                write!(
                    f,
                    "ZeroMQ message did not include expected <IDS|MSG> delimiter"
                )
            }
        }
    }
}

impl<T> JupyterMessage<T> {
    /// Parse a Jupyter message from an array of buffers (from a ZeroMQ message)
    pub fn from_buffers(bufs: Vec<Vec<u8>>) -> Result<JupyterMessage<T>, MessageError> {
        let mut iter = bufs.iter();

        // Find the position of the <IDS|MSG> delimiter in the message, which
        // separates the socket identities (IDS) from the body of the message.
        if let Some(pos) = iter.position(|buf| &buf[..] == MSG_DELIM) {
            return JupyterMessage::from_msg_bufs(bufs[pos + 1..].to_vec());
        }

        // No delimiter found.
        return Err(MessageError::MissingDelimiter);
    }

    fn from_msg_bufs(mut bufs: Vec<Vec<u8>>) -> Result<JupyterMessage<T>, MessageError> {
        // TODO
        Err(MessageError::MissingDelimiter)
    }
}
