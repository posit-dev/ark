/*
 * jupyter_message.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::session::Session;
use crate::socket::signed_socket::SignedSocket;
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::header::JupyterHeader;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;
use crate::wire::status::KernelStatus;
use crate::wire::wire_message::WireMessage;
use log::trace;
use serde::{Deserialize, Serialize};
use std::rc::Rc;

/// Represents a Jupyter message
#[derive(Debug, Clone)]
pub struct JupyterMessage<T> {
    /// The ZeroMQ identities (for ROUTER sockets)
    pub zmq_identities: Vec<Vec<u8>>,

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
    fn socket_type() -> SocketType;
}

pub enum SocketType {
    Shell,
    Heartbeat,
    IOPub,
    Stdin,
    Control,
}

/// Convenience trait for grouping traits that must be present on all Jupyter
/// protocol messages
pub trait ProtocolMessage: MessageType + Serialize + std::fmt::Debug + Clone {}
impl<T> ProtocolMessage for T where T: MessageType + Serialize + std::fmt::Debug + Clone {}

/// List of all known/implemented messages
#[derive(Debug)]
pub enum Message {
    KernelInfoRequest(JupyterMessage<KernelInfoRequest>),
    KernelInfoReply(JupyterMessage<KernelInfoReply>),
    IsCompleteReply(JupyterMessage<IsCompleteReply>),
    IsCompleteRequest(JupyterMessage<IsCompleteRequest>),
    ExecuteRequest(JupyterMessage<ExecuteRequest>),
    ExecuteReply(JupyterMessage<ExecuteReply>),
    CompleteRequest(JupyterMessage<CompleteRequest>),
    CompleteReply(JupyterMessage<CompleteReply>),
    Status(JupyterMessage<KernelStatus>),
}

/// Represents status returned from kernel inside messages.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Error,
}

impl Message {
    /// Converts from a wire message to a Jupyter message by examining the message
    /// type and attempting to coerce the content into the appropriate
    /// structure.
    pub fn to_jupyter_message(msg: WireMessage) -> Result<Self, Error> {
        let kind = msg.header.msg_type.clone();
        if kind == KernelInfoRequest::message_type() {
            return Ok(Message::KernelInfoRequest(msg.to_message_type()?));
        } else if kind == KernelInfoReply::message_type() {
            return Ok(Message::KernelInfoReply(msg.to_message_type()?));
        } else if kind == IsCompleteRequest::message_type() {
            return Ok(Message::IsCompleteRequest(msg.to_message_type()?));
        } else if kind == IsCompleteReply::message_type() {
            return Ok(Message::IsCompleteReply(msg.to_message_type()?));
        } else if kind == ExecuteRequest::message_type() {
            return Ok(Message::ExecuteRequest(msg.to_message_type()?));
        } else if kind == ExecuteReply::message_type() {
            return Ok(Message::ExecuteReply(msg.to_message_type()?));
        } else if kind == CompleteRequest::message_type() {
            return Ok(Message::CompleteRequest(msg.to_message_type()?));
        } else if kind == CompleteReply::message_type() {
            return Ok(Message::CompleteReply(msg.to_message_type()?));
        } else if kind == KernelStatus::message_type() {
            return Ok(Message::Status(msg.to_message_type()?));
        }
        return Err(Error::UnknownMessageType(kind));
    }

    pub fn read_from_socket(socket: &SignedSocket) -> Result<Self, Error> {
        let msg = WireMessage::read_from_socket(socket)?;
        Message::to_jupyter_message(msg)
    }
}

impl<T> JupyterMessage<T>
where
    T: ProtocolMessage,
{
    pub fn send(self, socket: &SignedSocket) -> Result<(), Error> {
        trace!("Sending Jupyter message to front end: {:?}", self);
        let msg = WireMessage::from_jupyter_message(self)?;
        msg.send(socket)?;
        Ok(())
    }

    pub fn create(
        content: T,
        parent: Option<JupyterHeader>,
        session: &Session,
    ) -> JupyterMessage<T> {
        JupyterMessage::<T> {
            zmq_identities: Vec::new(),
            header: JupyterHeader::create(
                T::message_type(),
                session.session_id.clone(),
                session.username.clone(),
            ),
            parent_header: parent,
            content: content,
        }
    }

    pub fn send_reply<R: ProtocolMessage>(
        &self,
        content: R,
        socket: &SignedSocket,
    ) -> Result<(), Error> {
        let reply = self.reply_msg(content, &socket.session)?;
        reply.send(&socket)
    }

    pub fn reply_msg<R: ProtocolMessage>(
        &self,
        content: R,
        session: &Session,
    ) -> Result<WireMessage, Error> {
        let reply = self.create_reply(content, session);
        WireMessage::from_jupyter_message(reply)
    }

    pub fn create_reply<R: ProtocolMessage>(
        &self,
        content: R,
        session: &Session,
    ) -> JupyterMessage<R> {
        JupyterMessage::<R> {
            zmq_identities: self.zmq_identities.clone(),
            header: JupyterHeader::create(
                R::message_type(),
                session.session_id.clone(),
                session.username.clone(),
            ),
            parent_header: Some(self.header.clone()),
            content: content,
        }
    }
}
