/*
 * jupyter_message.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use super::display_data::DisplayData;
use super::handshake_reply::HandshakeReply;
use super::handshake_request::HandshakeRequest;
use super::kernel_info_full_reply::KernelInfoReply;
use super::stream::StreamOutput;
use super::update_display_data::UpdateDisplayData;
use super::welcome::Welcome;
use crate::comm::base_comm::JsonRpcReply;
use crate::comm::ui_comm::UiFrontendRequest;
use crate::error::Error;
use crate::session::Session;
use crate::socket::socket::Socket;
use crate::wire::comm_close::CommClose;
use crate::wire::comm_info_reply::CommInfoReply;
use crate::wire::comm_info_request::CommInfoRequest;
use crate::wire::comm_msg::CommWireMsg;
use crate::wire::comm_open::CommOpen;
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::error_reply::ErrorReply;
use crate::wire::exception::Exception;
use crate::wire::execute_error::ExecuteError;
use crate::wire::execute_input::ExecuteInput;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_reply_exception::ExecuteReplyException;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::execute_result::ExecuteResult;
use crate::wire::header::JupyterHeader;
use crate::wire::input_reply::InputReply;
use crate::wire::input_request::InputRequest;
use crate::wire::inspect_reply::InspectReply;
use crate::wire::inspect_request::InspectRequest;
use crate::wire::interrupt_reply::InterruptReply;
use crate::wire::interrupt_request::InterruptRequest;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::kernel_info_request::KernelInfoRequest;
use crate::wire::originator::Originator;
use crate::wire::shutdown_request::ShutdownRequest;
use crate::wire::status::KernelStatus;
use crate::wire::wire_message::WireMessage;

/// Represents a Jupyter message
#[derive(Debug, Clone)]
pub struct JupyterMessage<T> {
    /// The ZeroMQ identities (for ROUTER sockets)
    pub zmq_identities: Vec<Vec<u8>>,

    /// The header for this message
    pub header: JupyterHeader,

    /// The header of the message from which this message originated. Optional;
    /// not all messages have a parent.
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
    // Shell
    KernelInfoReply(JupyterMessage<KernelInfoReply>),
    KernelInfoRequest(JupyterMessage<KernelInfoRequest>),
    CompleteReply(JupyterMessage<CompleteReply>),
    CompleteRequest(JupyterMessage<CompleteRequest>),
    ExecuteReply(JupyterMessage<ExecuteReply>),
    ExecuteReplyException(JupyterMessage<ExecuteReplyException>),
    ExecuteRequest(JupyterMessage<ExecuteRequest>),
    InspectReply(JupyterMessage<InspectReply>),
    InspectRequest(JupyterMessage<InspectRequest>),
    IsCompleteReply(JupyterMessage<IsCompleteReply>),
    IsCompleteRequest(JupyterMessage<IsCompleteRequest>),
    CommInfoReply(JupyterMessage<CommInfoReply>),
    CommInfoRequest(JupyterMessage<CommInfoRequest>),
    CommRequest(JupyterMessage<UiFrontendRequest>),
    CommReply(JupyterMessage<JsonRpcReply>),
    InputReply(JupyterMessage<InputReply>),
    InputRequest(JupyterMessage<InputRequest>),
    // Control
    InterruptReply(JupyterMessage<InterruptReply>),
    InterruptRequest(JupyterMessage<InterruptRequest>),
    ShutdownRequest(JupyterMessage<ShutdownRequest>),
    // Registration
    HandshakeRequest(JupyterMessage<HandshakeRequest>),
    HandshakeReply(JupyterMessage<HandshakeReply>),
    // IOPub
    Status(JupyterMessage<KernelStatus>),
    ExecuteResult(JupyterMessage<ExecuteResult>),
    ExecuteError(JupyterMessage<ExecuteError>),
    ExecuteInput(JupyterMessage<ExecuteInput>),
    Stream(JupyterMessage<StreamOutput>),
    DisplayData(JupyterMessage<DisplayData>),
    UpdateDisplayData(JupyterMessage<UpdateDisplayData>),
    Welcome(JupyterMessage<Welcome>),
    // IOPub/Shell
    CommMsg(JupyterMessage<CommWireMsg>),
    CommOpen(JupyterMessage<CommOpen>),
    CommClose(JupyterMessage<CommClose>),
}

/// Associates a `Message` to a 0MQ socket.
///
/// At a high level, outbound messages originate from kernel components on a
/// crossbeam channel and are transfered to the client via a 0MQ socket owned by
/// the forwarding thread.
pub enum OutboundMessage {
    StdIn(Message),
    IOPub(Message),
}

/// Represents status returned from kernel inside messages.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Error,
}

/// Conversion from a `Message` to a `WireMessage`; used to send messages over a
/// socket
impl TryFrom<&Message> for WireMessage {
    type Error = crate::error::Error;

    fn try_from(msg: &Message) -> Result<Self, Error> {
        match msg {
            Message::CompleteReply(msg) => WireMessage::try_from(msg),
            Message::CompleteRequest(msg) => WireMessage::try_from(msg),
            Message::ExecuteReply(msg) => WireMessage::try_from(msg),
            Message::ExecuteReplyException(msg) => WireMessage::try_from(msg),
            Message::ExecuteRequest(msg) => WireMessage::try_from(msg),
            Message::ExecuteResult(msg) => WireMessage::try_from(msg),
            Message::ExecuteError(msg) => WireMessage::try_from(msg),
            Message::ExecuteInput(msg) => WireMessage::try_from(msg),
            Message::InputReply(msg) => WireMessage::try_from(msg),
            Message::InputRequest(msg) => WireMessage::try_from(msg),
            Message::InspectReply(msg) => WireMessage::try_from(msg),
            Message::InspectRequest(msg) => WireMessage::try_from(msg),
            Message::InterruptReply(msg) => WireMessage::try_from(msg),
            Message::InterruptRequest(msg) => WireMessage::try_from(msg),
            Message::IsCompleteReply(msg) => WireMessage::try_from(msg),
            Message::IsCompleteRequest(msg) => WireMessage::try_from(msg),
            Message::KernelInfoReply(msg) => WireMessage::try_from(msg),
            Message::KernelInfoRequest(msg) => WireMessage::try_from(msg),
            Message::ShutdownRequest(msg) => WireMessage::try_from(msg),
            Message::Status(msg) => WireMessage::try_from(msg),
            Message::CommInfoReply(msg) => WireMessage::try_from(msg),
            Message::CommInfoRequest(msg) => WireMessage::try_from(msg),
            Message::CommOpen(msg) => WireMessage::try_from(msg),
            Message::CommMsg(msg) => WireMessage::try_from(msg),
            Message::CommClose(msg) => WireMessage::try_from(msg),
            Message::CommRequest(msg) => WireMessage::try_from(msg),
            Message::CommReply(msg) => WireMessage::try_from(msg),
            Message::Stream(msg) => WireMessage::try_from(msg),
            Message::HandshakeReply(msg) => WireMessage::try_from(msg),
            Message::HandshakeRequest(msg) => WireMessage::try_from(msg),
            Message::DisplayData(msg) => WireMessage::try_from(msg),
            Message::UpdateDisplayData(msg) => WireMessage::try_from(msg),
            Message::Welcome(msg) => WireMessage::try_from(msg),
        }
    }
}

impl TryFrom<&WireMessage> for Message {
    type Error = crate::error::Error;

    /// Converts from a wire message to a Jupyter message by examining the message
    /// type and attempting to coerce the content into the appropriate
    /// structure.
    ///
    /// Note that not all message types are supported here; this handles only
    /// messages that are received from the frontend.
    fn try_from(msg: &WireMessage) -> Result<Self, Error> {
        let kind = msg.header.msg_type.clone();

        if kind == KernelInfoRequest::message_type() {
            return Ok(Message::KernelInfoRequest(JupyterMessage::try_from(msg)?));
        }
        if kind == KernelInfoReply::message_type() {
            return Ok(Message::KernelInfoReply(JupyterMessage::try_from(msg)?));
        }
        if kind == IsCompleteRequest::message_type() {
            return Ok(Message::IsCompleteRequest(JupyterMessage::try_from(msg)?));
        }
        if kind == IsCompleteReply::message_type() {
            return Ok(Message::IsCompleteReply(JupyterMessage::try_from(msg)?));
        }
        if kind == InspectRequest::message_type() {
            return Ok(Message::InspectRequest(JupyterMessage::try_from(msg)?));
        }
        if kind == InspectReply::message_type() {
            return Ok(Message::InspectReply(JupyterMessage::try_from(msg)?));
        }
        if kind == ExecuteReplyException::message_type() {
            if let Ok(data) = JupyterMessage::try_from(msg) {
                return Ok(Message::ExecuteReplyException(data));
            }
            // else fallthrough to try `ExecuteRequest` which has the same message type
        }
        if kind == ExecuteRequest::message_type() {
            return Ok(Message::ExecuteRequest(JupyterMessage::try_from(msg)?));
        }
        if kind == ExecuteReply::message_type() {
            return Ok(Message::ExecuteReply(JupyterMessage::try_from(msg)?));
        }
        if kind == ExecuteResult::message_type() {
            return Ok(Message::ExecuteResult(JupyterMessage::try_from(msg)?));
        }
        if kind == ExecuteError::message_type() {
            return Ok(Message::ExecuteError(JupyterMessage::try_from(msg)?));
        }
        if kind == ExecuteInput::message_type() {
            return Ok(Message::ExecuteInput(JupyterMessage::try_from(msg)?));
        }
        if kind == CompleteRequest::message_type() {
            return Ok(Message::CompleteRequest(JupyterMessage::try_from(msg)?));
        }
        if kind == CompleteReply::message_type() {
            return Ok(Message::CompleteReply(JupyterMessage::try_from(msg)?));
        }
        if kind == DisplayData::message_type() {
            return Ok(Message::DisplayData(JupyterMessage::try_from(msg)?));
        }
        if kind == UpdateDisplayData::message_type() {
            return Ok(Message::UpdateDisplayData(JupyterMessage::try_from(msg)?));
        }
        if kind == ShutdownRequest::message_type() {
            return Ok(Message::ShutdownRequest(JupyterMessage::try_from(msg)?));
        }
        if kind == KernelStatus::message_type() {
            return Ok(Message::Status(JupyterMessage::try_from(msg)?));
        }
        if kind == CommInfoRequest::message_type() {
            return Ok(Message::CommInfoRequest(JupyterMessage::try_from(msg)?));
        }
        if kind == CommInfoReply::message_type() {
            return Ok(Message::CommInfoReply(JupyterMessage::try_from(msg)?));
        }
        if kind == CommOpen::message_type() {
            return Ok(Message::CommOpen(JupyterMessage::try_from(msg)?));
        }
        if kind == CommWireMsg::message_type() {
            return Ok(Message::CommMsg(JupyterMessage::try_from(msg)?));
        }
        if kind == CommClose::message_type() {
            return Ok(Message::CommClose(JupyterMessage::try_from(msg)?));
        }
        if kind == InterruptRequest::message_type() {
            return Ok(Message::InterruptRequest(JupyterMessage::try_from(msg)?));
        }
        if kind == InterruptReply::message_type() {
            return Ok(Message::InterruptReply(JupyterMessage::try_from(msg)?));
        }
        if kind == InputReply::message_type() {
            return Ok(Message::InputReply(JupyterMessage::try_from(msg)?));
        }
        if kind == InputRequest::message_type() {
            return Ok(Message::InputRequest(JupyterMessage::try_from(msg)?));
        }
        if kind == StreamOutput::message_type() {
            return Ok(Message::Stream(JupyterMessage::try_from(msg)?));
        }
        if kind == UiFrontendRequest::message_type() {
            return Ok(Message::CommRequest(JupyterMessage::try_from(msg)?));
        }
        if kind == JsonRpcReply::message_type() {
            return Ok(Message::CommReply(JupyterMessage::try_from(msg)?));
        }
        if kind == HandshakeRequest::message_type() {
            return Ok(Message::HandshakeRequest(JupyterMessage::try_from(msg)?));
        }
        if kind == HandshakeReply::message_type() {
            return Ok(Message::HandshakeReply(JupyterMessage::try_from(msg)?));
        }
        if kind == Welcome::message_type() {
            return Ok(Message::Welcome(JupyterMessage::try_from(msg)?));
        }
        return Err(Error::UnknownMessageType(kind));
    }
}

impl Message {
    pub fn read_from_socket(socket: &Socket) -> Result<Self, Error> {
        let msg = WireMessage::read_from_socket(socket)?;
        Message::try_from(&msg)
    }

    pub fn send(&self, socket: &Socket) -> Result<(), Error> {
        let msg = WireMessage::try_from(self)?;
        msg.send(socket)?;
        Ok(())
    }
}

impl<T> JupyterMessage<T>
where
    T: ProtocolMessage,
{
    /// Sends this Jupyter message to the designated ZeroMQ socket.
    pub fn send(self, socket: &Socket) -> Result<(), Error> {
        let msg = WireMessage::try_from(&self)?;
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
            content,
        }
    }

    /// Create a new Jupyter message with a specific ZeroMQ identity.
    pub fn create_with_identity(
        originator: Originator,
        content: T,
        session: &Session,
    ) -> JupyterMessage<T> {
        JupyterMessage::<T> {
            zmq_identities: originator.zmq_identities,
            header: JupyterHeader::create(
                T::message_type(),
                session.session_id.clone(),
                session.username.clone(),
            ),
            parent_header: Some(originator.header),
            content,
        }
    }

    /// Sends a reply to the message; convenience method combining creating the
    /// reply and sending it.
    pub fn send_reply<R: ProtocolMessage>(&self, content: R, socket: &Socket) -> crate::Result<()> {
        let reply = self.reply_msg(content, &socket.session)?;
        reply.send(&socket)
    }

    /// Sends an error reply to the message.
    pub fn send_error<R: ProtocolMessage>(
        &self,
        exception: Exception,
        socket: &Socket,
    ) -> crate::Result<()> {
        let reply = self.error_reply::<R>(exception, &socket.session);
        reply.send(&socket)
    }

    pub fn send_execute_error(
        &self,
        exception: Exception,
        exec_count: u32,
        socket: &Socket,
    ) -> crate::Result<()> {
        let rep = ExecuteReplyException {
            status: Status::Error,
            execution_count: exec_count,
            exception,
        };
        self.send_reply(rep, socket)
    }

    /// Create a raw reply message to this message.
    fn reply_msg<R: ProtocolMessage>(
        &self,
        content: R,
        session: &Session,
    ) -> Result<WireMessage, Error> {
        let reply = self.create_reply(content, session);
        WireMessage::try_from(&reply)
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
            content,
        }
    }

    /// Creates an error reply to this message; used on ROUTER/DEALER sockets to
    /// indicate that an error occurred while processing a Request message.
    ///
    /// Error replies are special cases; they use the message type of a
    /// successful reply, but their content is an Exception instead.
    pub fn error_reply<R: ProtocolMessage>(
        &self,
        exception: Exception,
        session: &Session,
    ) -> JupyterMessage<ErrorReply> {
        JupyterMessage::<ErrorReply> {
            zmq_identities: self.zmq_identities.clone(),
            header: JupyterHeader::create(
                R::message_type(),
                session.session_id.clone(),
                session.username.clone(),
            ),
            parent_header: Some(self.header.clone()),
            content: ErrorReply {
                status: Status::Error,
                exception,
            },
        }
    }
}
