/*
 * stdin.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::Receiver;
use crossbeam::channel::SendError;
use crossbeam::channel::Sender;
use crossbeam::select;
use futures::executor::block_on;
use log::error;
use log::trace;
use log::warn;

use crate::language::shell_handler::ShellHandler;
use crate::session::Session;
use crate::socket::iopub::IOPubContextChannel;
use crate::socket::iopub::IOPubMessage;
use crate::traits::iopub::IOPubSenderExt;
use crate::wire::input_reply::InputReply;
use crate::wire::input_request::ShellInputRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::OutboundMessage;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::originator::Originator;
use crate::wire::status::ExecutionState;

pub struct Stdin {
    /// Receiver connected to the StdIn's ZeroMQ socket
    inbound_rx: Receiver<Message>,

    /// Sender connected to the StdIn's ZeroMQ socket
    outbound_tx: Sender<OutboundMessage>,

    /// Language-provided shell handler object
    shell_handler: Arc<Mutex<dyn ShellHandler>>,

    // Sends messages to the IOPub socket. In particular for busy/idle updates
    // related to input replies so that new output gets attached to the correct
    // input element in the console.
    iopub_tx: Sender<IOPubMessage>,

    // 0MQ session, needed to create `JupyterMessage` objects
    session: Session,
}

impl Stdin {
    /// Create a new Stdin socket
    ///
    /// * `socket` - The underlying ZeroMQ socket
    /// * `shell_handler` - The language's shell handler
    /// * `iopub_tx` - The IOPub message sender
    pub fn new(
        inbound_rx: Receiver<Message>,
        outbound_tx: Sender<OutboundMessage>,
        shell_handler: Arc<Mutex<dyn ShellHandler>>,
        iopub_tx: Sender<IOPubMessage>,
        session: Session,
    ) -> Self {
        Self {
            inbound_rx,
            outbound_tx,
            shell_handler,
            iopub_tx,
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

            self.handle_input_reply(reply)
        }
    }

    fn send_state<T: ProtocolMessage>(
        &self,
        parent: JupyterMessage<T>,
        state: ExecutionState,
    ) -> Result<(), SendError<IOPubMessage>> {
        self.iopub_tx
            .send_state(parent, IOPubContextChannel::Shell, state)
    }

    // Mimics the structure of handling other messages in `Shell`. In
    // particular, toggling busy/idle states.
    fn handle_input_reply(&self, reply: JupyterMessage<InputReply>) {
        // Enter the kernel-busy state in preparation for handling the message.
        if let Err(err) = self.send_state(reply.clone(), ExecutionState::Busy) {
            warn!("Failed to change kernel status to busy: {err}");
        }

        // Send the reply to the shell handler
        let shell_handler = self.shell_handler.lock().unwrap();

        let orig = Originator::from(&reply);
        if let Err(err) = block_on(shell_handler.handle_input_reply(&reply.content, orig)) {
            warn!("Error handling input reply: {:?}", err);
        }

        // Return to idle -- we always do this, even if the message generated an
        // error, since many front ends won't submit additional messages until
        // the kernel is marked idle.
        if let Err(err) = self.send_state(reply, ExecutionState::Idle) {
            warn!("Failed to restore kernel status to idle: {err}");
        }
    }
}
