/*
 * stdin.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use stdext::result::ResultExt;

use crate::comm::base_comm::JsonRpcError;
use crate::comm::base_comm::JsonRpcErrorCode;
use crate::comm::base_comm::JsonRpcErrorData;
use crate::comm::base_comm::JsonRpcReply;
use crate::session::Session;
use crate::wire::input_reply::InputReply;
use crate::wire::input_request::InputRequest;
use crate::wire::input_request::ShellInputRequest;
use crate::wire::input_request::StdInRpcReply;
use crate::wire::input_request::UiCommFrontendRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::MessageType;
use crate::wire::jupyter_message::OutboundMessage;

pub enum StdInRequest {
    Input(ShellInputRequest),
    Comm(UiCommFrontendRequest),
}

enum StdInReplySender {
    Input(Sender<crate::Result<InputReply>>),
    Comm(Sender<StdInRpcReply>),
}

pub struct Stdin {
    /// Receiver connected to the StdIn's ZeroMQ socket
    inbound_rx: Receiver<crate::Result<Message>>,

    /// Sender connected to the StdIn's ZeroMQ socket
    outbound_tx: Sender<OutboundMessage>,

    // 0MQ session, needed to create `JupyterMessage` objects
    session: Session,
}

impl Stdin {
    /// Create a new Stdin socket
    ///
    /// * `inbound_rx` - Channel relaying replies from frontend
    /// * `outbound_tx` - Channel relaying requests to frontend
    /// * `session` - Juptyer session
    pub fn new(
        inbound_rx: Receiver<crate::Result<Message>>,
        outbound_tx: Sender<OutboundMessage>,
        session: Session,
    ) -> Self {
        Self {
            inbound_rx,
            outbound_tx,
            session,
        }
    }

    /// Listens for messages on the stdin socket. This follows a simple loop:
    ///
    /// 1. Wait for
    pub fn listen(
        &self,
        stdin_request_rx: Receiver<StdInRequest>,
        stdin_reply_tx: Sender<crate::Result<InputReply>>,
        interrupt_rx: Receiver<bool>,
    ) {
        'outer: loop {
            // Listen for input requests from the backend. We ignore
            // interrupt notifications here and loop infinitely over them.
            //
            // This could be simplified by having a mechanism for
            // subscribing and unsubscribing to a broadcasting channel. We
            // don't need to listen to interrupts at this stage so we'd
            // only subscribe after receiving an input request, and the
            // loop/select below could be removed.
            //
            // If any of the channels disconnects, all senders have been
            // dropped, which signals shutdown. Exit cleanly rather than
            // spinning on the disconnected channel.
            let req: StdInRequest;
            loop {
                select! {
                    recv(stdin_request_rx) -> msg => {
                        match msg {
                            Ok(m) => {
                                req = m;
                                break;
                            },
                            Err(err) => {
                                log::trace!("StdIn shutting down: request channel disconnected: {err:?}");
                                return;
                            }
                        }
                    },
                    recv(interrupt_rx) -> msg => {
                        if let Err(err) = msg {
                            log::trace!("StdIn shutting down: interrupt channel disconnected: {err:?}");
                            return;
                        }
                        continue;
                    }
                };
            }

            // Keep the request's ID to match comm replies to this request
            let (request, request_id, reply_tx) = match req {
                StdInRequest::Input(req) => {
                    let req = JupyterMessage::create_with_identity(
                        req.originator,
                        req.request,
                        &self.session,
                    );
                    let id = req.header.msg_id.clone();
                    (
                        Message::InputRequest(req),
                        id,
                        StdInReplySender::Input(stdin_reply_tx.clone()),
                    )
                },
                StdInRequest::Comm(comm_req) => {
                    // This is a request to the frontend
                    let req = JupyterMessage::create_with_identity(
                        comm_req.originator,
                        comm_req.request,
                        &self.session,
                    );
                    let id = req.header.msg_id.clone();
                    (
                        Message::CommRequest(req),
                        id,
                        StdInReplySender::Comm(comm_req.reply_tx),
                    )
                },
            };

            // Deliver the message to the frontend
            if let Err(err) = self.outbound_tx.send(OutboundMessage::StdIn(request)) {
                log::error!("Failed to send message to frontend: {}", err);
            }
            log::trace!("Sent input request to frontend, waiting for input reply...");

            // Wait for the frontend's reply to this request. A reply to an
            // earlier request can arrive first, for instance when an
            // interrupt stopped the wait for it. Skip every message that is
            // not the reply to this request.
            loop {
                let message = select! {
                    recv(self.inbound_rx) -> msg => match msg {
                        Ok(m) => m,
                        Err(err) => {
                            log::trace!("StdIn shutting down: inbound socket disconnected: {err:?}");
                            return;
                        }
                    },
                    // Cancel current iteration if an interrupt is
                    // signaled. We're no longer waiting for an `input_reply`
                    // but for an `input_request`.
                    recv(interrupt_rx) -> msg => {
                        if let Err(err) = msg {
                            log::trace!("StdIn shutting down: interrupt channel disconnected: {err:?}");
                            return;
                        }
                        log::trace!("Received interrupt signal in StdIn");

                        match reply_tx {
                            StdInReplySender::Input(_tx) => {
                                // Nothing to do since `read_console()` will detect
                                // the interrupt independently. Fall through.
                            },
                            StdInReplySender::Comm(tx) => {
                                tx.send(StdInRpcReply::Interrupt).log_err();
                            },
                        }

                        continue 'outer;
                    }
                };

                log::trace!("Received reply from front-end: {message:?}");

                match Self::deliver_reply(message, &reply_tx, &request_id) {
                    None => break,
                    Some(message) => {
                        log::warn!("Ignoring unexpected message on stdin socket: {message:?}")
                    },
                }
            }
        }
    }

    /// Sends `message` to the pending request if it is the reply to that
    /// request, and returns `None`. Otherwise returns `message` unchanged. The
    /// reply must have the type the request expects. A comm reply with a
    /// parent header must also be parented to the request. An input reply
    /// parented to another input request is a late reply to that request, so
    /// it is skipped too. Other input replies are accepted whatever their
    /// parent, because frontends differ in what they parent them to, and
    /// some send no parent at all. A deserialization error goes to the
    /// pending request.
    fn deliver_reply(
        message: crate::Result<Message>,
        reply_tx: &StdInReplySender,
        request_id: &str,
    ) -> Option<crate::Result<Message>> {
        match (message, reply_tx) {
            (Ok(Message::InputReply(reply)), StdInReplySender::Input(tx))
                if !reply.parent_header.as_ref().is_some_and(|parent| {
                    parent.msg_type == InputRequest::message_type() && parent.msg_id != request_id
                }) =>
            {
                tx.send(Ok(reply.content)).log_err();
            },
            (Ok(Message::CommReply(reply)), StdInReplySender::Comm(tx))
                if reply
                    .parent_header
                    .as_ref()
                    .is_none_or(|parent| parent.msg_id == request_id) =>
            {
                tx.send(StdInRpcReply::Reply(reply.content)).log_err();
            },
            // Might be an unserialisation error. Propagate the error to R.
            (Err(err), StdInReplySender::Input(tx)) => {
                tx.send(Err(err)).log_err();
            },
            (Err(err), StdInReplySender::Comm(tx)) => {
                let resp = StdInRpcReply::Reply(JsonRpcReply::Error(JsonRpcError {
                    error: JsonRpcErrorData {
                        message: format!("Error while receiving frontend response: {err}"),
                        code: JsonRpcErrorCode::InternalError,
                    },
                }));
                tx.send(resp).log_err();
            },
            (message, _) => return Some(message),
        }
        None
    }
}
