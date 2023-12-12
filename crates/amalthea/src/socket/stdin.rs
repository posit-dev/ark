/*
 * stdin.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use log::error;
use log::trace;
use log::warn;

use crate::comm::comm_channel::RpcRequest;
use crate::comm::frontend_comm::FrontendRpcResponse;
use crate::session::Session;
use crate::wire::input_reply::InputReply;
use crate::wire::input_request::ShellInputRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::OutboundMessage;
use crate::wire::originator::Originator;

pub enum StdInRequest {
    InputRequest(ShellInputRequest),
    CommRequest(Originator, Sender<FrontendRpcResponse>, RpcRequest),
}

enum StdInReplySender {
    Input(Sender<InputReply>),
    Comm(Sender<FrontendRpcResponse>),
}

pub struct Stdin {
    /// Receiver connected to the StdIn's ZeroMQ socket
    inbound_rx: Receiver<Message>,

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
        inbound_rx: Receiver<Message>,
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
        input_reply_tx: Sender<InputReply>,
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
                                error!("Could not read input request: {}", err);
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
                StdInRequest::InputRequest(req) => {
                    let req = Message::InputRequest(JupyterMessage::create_with_identity(
                        req.originator,
                        req.request,
                        &self.session,
                    ));
                    (req, StdInReplySender::Input(input_reply_tx.clone()))
                },
                StdInRequest::CommRequest(orig, response_tx, req) => {
                    // This is a request from to the frontend
                    let req = Message::CommRequest(JupyterMessage::create_with_identity(
                        Some(orig),
                        req,
                        &self.session,
                    ));
                    (req, StdInReplySender::Comm(response_tx))
                },
            };

            // Deliver the message to the front end
            if let Err(err) = self.outbound_tx.send(OutboundMessage::StdIn(request)) {
                error!("Failed to send message to front end: {}", err);
            }
            trace!("Sent input request to front end, waiting for input reply...");

            // Wait for the front end's reply message from the ZeroMQ socket.
            let message = select! {
                recv(self.inbound_rx) -> msg => match msg {
                    Ok(m) => m,
                    Err(err) => {
                        error!("Could not read message from stdin socket: {}", err);
                        continue;
                    }
                },
                // Cancel current iteration if an interrupt is
                // signaled. We're no longer waiting for an `input_reply`
                // but for an `input_request`.
                recv(interrupt_rx) -> msg => {
                    if let Err(err) = msg {
                        error!("Could not read interrupt message: {}", err);
                    }
                    continue;
                }
            };

            trace!("Received reply from front-end: {:?}", message);

            // Only input and comm RPC replies are expected on this socket
            match message {
                Message::InputReply(ref reply) => {
                    if let StdInReplySender::Input(tx) = reply_tx {
                        tx.send(reply.content.clone()).unwrap();
                        continue;
                    }
                },
                Message::CommReply(ref reply) => {
                    if let StdInReplySender::Comm(tx) = reply_tx {
                        tx.send(reply.content.clone()).unwrap();
                        continue;
                    }
                },
                _ => {},
            };

            warn!("Received unexpected message on stdin socket: {:?}", message);
        }
    }
}
