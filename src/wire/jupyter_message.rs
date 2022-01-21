/*
 * jupyter_message.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::header::JupyterHeader;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::wire_message::WireMessage;
use serde::de::DeserializeOwned;
use std::fmt;

/// Represents a Jupyter message
pub struct JupyterMessage<T> {
    /// The header for this message
    pub header: JupyterHeader,

    /// The header of the message from which this message originated
    pub parent_header: JupyterHeader,

    /// The body (payload) of the message
    pub content: T,
}

pub enum Message {
    KernelInfoRequest,
    KernelInfoReply(JupyterMessage<KernelInfoReply>),
}

pub enum Error {
    InvalidMessage(serde_json::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::InvalidMessage(err) => {
                write!(f, "Message content invalid: {}", err)
            }
        }
    }
}

impl<T> JupyterMessage<T>
where
    T: DeserializeOwned,
{
    pub fn from_wire(wire: WireMessage) -> Result<JupyterMessage<T>, Error> {
        match serde_json::from_value(wire.content) {
            Ok(content) => Ok(Self {
                header: wire.header,
                parent_header: wire.parent_header,
                content: content,
            }),
            Err(err) => Err(Error::InvalidMessage(err)),
        }
    }
}
