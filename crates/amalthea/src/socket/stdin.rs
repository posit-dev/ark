/*
 * stdin.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use futures::executor::block_on;
use log::error;
use log::trace;
use log::warn;

use crate::language::shell_handler::ShellHandler;
use crate::session::Session;
use crate::wire::header::JupyterHeader;
use crate::wire::input_request::ShellInputRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::OutboundMessage;
use crate::wire::originator::Originator;

pub struct Stdin {
    /// Receiver connected to the StdIn's ZeroMQ socket
    inbound_rx: Receiver<Message>,

    /// Sender connected to the StdIn's ZeroMQ socket
    outbound_tx: Sender<OutboundMessage>,

    /// Language-provided shell handler object
    handler: Arc<Mutex<dyn ShellHandler>>,

    // IOPub message context. Updated from StdIn on input replies so that new
    // output gets attached to the correct input element in the console.
    msg_context: Arc<Mutex<Option<JupyterHeader>>>,

    // 0MQ session, needed to create `JupyterMessage` objects
    session: Session,
}

impl Stdin {
    /// Create a new Stdin socket
    ///
    /// * `socket` - The underlying ZeroMQ socket
    /// * `handler` - The language's shell handler
    /// * `msg_context` - The IOPub message context
    pub fn new(
        inbound_rx: Receiver<Message>,
        outbound_tx: Sender<OutboundMessage>,
        handler: Arc<Mutex<dyn ShellHandler>>,
        msg_context: Arc<Mutex<Option<JupyterHeader>>>,
        session: Session,
    ) -> Self {
        Self {
            inbound_rx,
            outbound_tx,
            handler,
            msg_context,
            session,
        }
    }

    /// Listens for messages on the stdin socket. This follows a simple loop:
    ///
    /// 1. Wait for
    pub fn listen(
        &self,
        input_request_rx: Receiver<ShellInputRequest>,
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
            let req: ShellInputRequest;
            loop {
                select! {
                    recv(input_request_rx) -> msg => {
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

            if let None = req.originator {
                warn!("No originator for stdin request");
            }

            // Deliver the message to the front end
            let msg = Message::InputRequest(JupyterMessage::create_with_identity(
                req.originator,
                req.request,
                &self.session,
            ));

            if let Err(err) = self.outbound_tx.send(OutboundMessage::StdIn(msg)) {
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

            // Only input replies are expected on this socket
            let reply = match message {
                Message::InputReply(reply) => reply,
                _ => {
                    warn!("Received unexpected message on stdin socket: {:?}", message);
                    continue;
                },
            };
            trace!("Received input reply from front-end: {:?}", reply);

            // Update IOPub message context
            {
                let mut ctxt = self.msg_context.lock().unwrap();
                *ctxt = Some(reply.header.clone());
            }

            // Send the reply to the shell handler
            let handler = self.handler.lock().unwrap();
            let orig = Originator::from(&reply);
            if let Err(err) = block_on(handler.handle_input_reply(&reply.content, orig)) {
                warn!("Error handling input reply: {:?}", err);
            }
        }
    }
}
