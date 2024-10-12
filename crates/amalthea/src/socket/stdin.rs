/*
 * stdin.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;

use crate::comm::base_comm::JsonRpcError;
use crate::comm::base_comm::JsonRpcErrorCode;
use crate::comm::base_comm::JsonRpcErrorData;
use crate::comm::base_comm::JsonRpcReply;
use crate::session::Session;
use crate::wire::input_reply::InputReply;
use crate::wire::input_request::ShellInputRequest;
use crate::wire::input_request::StdInRpcReply;
use crate::wire::input_request::UiCommFrontendRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
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
        loop {
            // Listen for input requests from the backend. We ignore
            // interrupt notifications here and loop infinitely over them.
            //
            // This could be simplified by having a mechanism for
            // subscribing and unsubscribing to a broadcasting channel. We
            // don't need to listen to interrupts at this stage so we'd
            // only subscribe after receiving an input request, and the
            // loop/select below could be removed.
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
                                log::error!("Could not read input request: {}", err);
                                continue;
                            }
                        }
                    },
                    recv(interrupt_rx) -> _ => {
                        continue;
                    }
                };
            }

            let (request, reply_tx) = match req {
                StdInRequest::Input(req) => {
                    let req = Message::InputRequest(JupyterMessage::create_with_identity(
                        req.originator,
                        req.request,
                        &self.session,
                    ));
                    (req, StdInReplySender::Input(stdin_reply_tx.clone()))
                },
                StdInRequest::Comm(comm_req) => {
                    // This is a request to the frontend
                    let req = Message::CommRequest(JupyterMessage::create_with_identity(
                        comm_req.originator,
                        comm_req.request,
                        &self.session,
                    ));
                    (req, StdInReplySender::Comm(comm_req.reply_tx))
                },
            };

            // Deliver the message to the frontend
            if let Err(err) = self.outbound_tx.send(OutboundMessage::StdIn(request)) {
                log::error!("Failed to send message to frontend: {}", err);
            }
            log::trace!("Sent input request to frontend, waiting for input reply...");

            // Wait for the frontend's reply message from the ZeroMQ socket.
            let message = select! {
                recv(self.inbound_rx) -> msg => match msg {
                    Ok(m) => m,
                    Err(err) => {
                        log::error!("Could not read message from stdin socket: {err:?}");
                        continue;
                    }
                },
                // Cancel current iteration if an interrupt is
                // signaled. We're no longer waiting for an `input_reply`
                // but for an `input_request`.
                recv(interrupt_rx) -> msg => {
                    log::trace!("Received interrupt signal in StdIn");

                    if let Err(err) = msg {
                        log::error!("Could not read interrupt message: {err:?}");
                    }

                    match reply_tx {
                        StdInReplySender::Input(_tx) => {
                            // Nothing to do since `read_console()` will detect
                            // the interrupt independently. Fall through.
                        },
                        StdInReplySender::Comm(tx) => {
                            tx.send(StdInRpcReply::Interrupt).unwrap();
                        },
                    }

                    continue;
                }
            };

            log::trace!("Received reply from front-end: {message:?}");

            // Only input and comm RPC replies are expected on this socket
            match message {
                Ok(ref message) => match message {
                    Message::InputReply(ref reply) => {
                        if let StdInReplySender::Input(tx) = reply_tx {
                            tx.send(Ok(reply.content.clone())).unwrap();
                            continue;
                        }
                    },
                    Message::CommReply(ref reply) => {
                        if let StdInReplySender::Comm(tx) = reply_tx {
                            let resp = StdInRpcReply::Reply(reply.content.clone());
                            tx.send(resp).unwrap();
                            continue;
                        }
                    },
                    _ => {
                        log::warn!("Unexpected message type {message:?} on StdIn",);
                        continue;
                    },
                },
                Err(err) => {
                    // Might be an unserialisation error. Propagate the error to R.
                    match reply_tx {
                        StdInReplySender::Input(tx) => {
                            tx.send(Err(err)).unwrap();
                        },
                        StdInReplySender::Comm(tx) => {
                            let resp = StdInRpcReply::Reply(JsonRpcReply::Error(JsonRpcError {
                                error: JsonRpcErrorData {
                                    message: format!(
                                        "Error while receiving frontend response: {err}"
                                    ),
                                    code: JsonRpcErrorCode::InternalError,
                                },
                            }));
                            tx.send(resp).unwrap();
                        },
                    }
                    continue;
                },
            };

            log::warn!("Received unexpected message on stdin socket: {message:?}");
        }
    }
}
