/*
 * jupyter_message.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::header::JupyterHeader;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;
use crate::wire::wire_message::MessageError;
use crate::wire::wire_message::WireMessage;
use hmac::Hmac;
use log::trace;
use serde::Serialize;
use sha2::Sha256;

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
    IsCompleteReply(JupyterMessage<IsCompleteReply>),
    IsCompleteRequest(JupyterMessage<IsCompleteRequest>),
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
        } else if kind == IsCompleteRequest::message_type() {
            return Ok(Message::IsCompleteRequest(msg.to_message_type()?));
        } else if kind == IsCompleteReply::message_type() {
            return Ok(Message::IsCompleteReply(msg.to_message_type()?));
        }
        return Err(MessageError::UnknownType(kind));
    }
}

impl<T> JupyterMessage<T>
where
    T: Serialize + MessageType + std::fmt::Debug,
{
    pub fn create(
        from: T,
        parent: Option<JupyterHeader>,
        username: String,
        session: String,
    ) -> Self {
        Self {
            header: JupyterHeader::create(T::message_type(), session, username),
            parent_header: parent,
            content: from,
        }
    }

    pub fn send(
        self,
        socket: &zmq::Socket,
        hmac: Option<Hmac<Sha256>>,
    ) -> Result<(), MessageError> {
        trace!("Sending Jupyter message to front end: {:?}", self);
        let msg = WireMessage::from_jupyter_message(self)?;
        msg.send(socket, hmac)?;
        Ok(())
    }

    pub fn create_reply<R: MessageType + Serialize>(&self, content: R) -> JupyterMessage<R> {
        JupyterMessage::<R> {
            header: JupyterHeader::create(
                R::message_type(),
                self.header.session.clone(),
                self.header.username.clone(),
            ),
            parent_header: Some(self.header.clone()),
            content: content,
        }
    }
}
