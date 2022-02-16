/*
 * jupyter_message.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::session::Session;
use crate::socket::socket::Socket;
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::execute_result::ExecuteResult;
use crate::wire::header::JupyterHeader;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;
use crate::wire::status::KernelStatus;
use crate::wire::wire_message::WireMessage;
use log::trace;
use serde::{Deserialize, Serialize};

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
    ExecuteResult(JupyterMessage<ExecuteResult>),
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

impl TryFrom<WireMessage> for Message {
    type Error = crate::error::Error;

    /// Converts from a wire message to a Jupyter message by examining the message
    /// type and attempting to coerce the content into the appropriate
    /// structure.
    fn try_from(msg: WireMessage) -> Result<Self, Error> {
        let kind = msg.header.msg_type.clone();
        if kind == KernelInfoRequest::message_type() {
            return Ok(Message::KernelInfoRequest(JupyterMessage::try_from(msg)?));
        } else if kind == KernelInfoReply::message_type() {
            return Ok(Message::KernelInfoReply(JupyterMessage::try_from(msg)?));
        } else if kind == IsCompleteRequest::message_type() {
            return Ok(Message::IsCompleteRequest(JupyterMessage::try_from(msg)?));
        } else if kind == IsCompleteReply::message_type() {
            return Ok(Message::IsCompleteReply(JupyterMessage::try_from(msg)?));
        } else if kind == ExecuteRequest::message_type() {
            return Ok(Message::ExecuteRequest(JupyterMessage::try_from(msg)?));
        } else if kind == ExecuteReply::message_type() {
            return Ok(Message::ExecuteReply(JupyterMessage::try_from(msg)?));
        } else if kind == ExecuteResult::message_type() {
            return Ok(Message::ExecuteResult(JupyterMessage::try_from(msg)?));
        } else if kind == CompleteRequest::message_type() {
            return Ok(Message::CompleteRequest(JupyterMessage::try_from(msg)?));
        } else if kind == CompleteReply::message_type() {
            return Ok(Message::CompleteReply(JupyterMessage::try_from(msg)?));
        } else if kind == KernelStatus::message_type() {
            return Ok(Message::Status(JupyterMessage::try_from(msg)?));
        }
        return Err(Error::UnknownMessageType(kind));
    }
}

impl Message {
    pub fn read_from_socket(socket: &Socket) -> Result<Self, Error> {
        let msg = WireMessage::read_from_socket(socket)?;
        Message::try_from(msg)
    }
}

impl<T> JupyterMessage<T>
where
    T: ProtocolMessage,
{
    /// Sends this Jupyter message to the designated ZeroMQ socket.
    pub fn send(self, socket: &Socket) -> Result<(), Error> {
        trace!("Sending Jupyter message to front end: {:?}", self);
        let msg = WireMessage::try_from(self)?;
        msg.send(socket)?;
        Ok(())
    }

    /// Create a new Jupyter message, optionally as a child (reply) to an
    /// existing message.
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

    /// Sends a reply to the message; convenience method combining creating the
    /// reply and sending it.
    pub fn send_reply<R: ProtocolMessage>(&self, content: R, socket: &Socket) -> Result<(), Error> {
        let reply = self.reply_msg(content, &socket.session)?;
        reply.send(&socket)
    }

    /// Create a raw reply message to this message.
    fn reply_msg<R: ProtocolMessage>(
        &self,
        content: R,
        session: &Session,
    ) -> Result<WireMessage, Error> {
        let reply = self.create_reply(content, session);
        WireMessage::try_from(reply)
    }

    /// Create a reply to this message with the given content.
    pub fn create_reply<R: ProtocolMessage>(
        &self,
        content: R,
        session: &Session,
    ) -> JupyterMessage<R> {
        // Note that the message we are creating needs to use the kernel session
        // (given as an argument), not the client session (which we could
        // otherwise copy from the message itself)
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
