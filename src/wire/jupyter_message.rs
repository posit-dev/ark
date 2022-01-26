/*
 * jupyter_message.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::header::JupyterHeader;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;
use crate::wire::wire_message::MessageError;
use crate::wire::wire_message::WireMessage;

/// Represents a Jupyter message
#[derive(Debug)]
pub struct JupyterMessage<T> {
    /// The header for this message
    pub header: JupyterHeader,

    /// The header of the message from which this message originated. Optional;
    /// not all messages have an originator.
    pub parent_header: Option<JupyterHeader>,

    /// The body (payload) of the message
    pub content: T,
}

/// Trait used to extract the wire message type from a Jupyter message
pub trait MessageType {
    fn message_type() -> String;
}

/// List of all known/implemented messages
#[derive(Debug)]
pub enum Message {
    KernelInfoRequest(JupyterMessage<KernelInfoRequest>),
    KernelInfoReply(JupyterMessage<KernelInfoReply>),
}

impl Message {
    /// Converts from a wire message to a Jupyter message by examining the message
    /// type and attempting to coerce the content into the appropriate
    /// structure.
    pub fn to_jupyter_message(msg: WireMessage) -> Result<Message, MessageError> {
        let kind = msg.header.msg_type.clone();
        if kind == KernelInfoRequest::message_type() {
            return Ok(Message::KernelInfoRequest(msg.to_message_type()?));
        } else if kind == KernelInfoReply::message_type() {
            return Ok(Message::KernelInfoReply(msg.to_message_type()?));
        }
        return Err(MessageError::UnknownType(kind));
    }
}
