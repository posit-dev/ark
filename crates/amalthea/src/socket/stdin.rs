/*
 * stdin.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::Receiver;
use futures::executor::block_on;
use log::trace;
use log::warn;

use crate::language::shell_handler::ShellHandler;
use crate::socket::socket::Socket;
use crate::wire::header::JupyterHeader;
use crate::wire::input_request::ShellInputRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::originator::Originator;

pub struct Stdin {
    /// The ZeroMQ stdin socket
    socket: Socket,

    /// Language-provided shell handler object
    handler: Arc<Mutex<dyn ShellHandler>>,

    // IOPub message context. Updated from StdIn on input replies so that new
    // output gets attached to the correct input element in the console.
    msg_context: Arc<Mutex<Option<JupyterHeader>>>,
}

impl Stdin {
    /// Create a new Stdin socket
    ///
    /// * `socket` - The underlying ZeroMQ socket
    /// * `handler` - The language's shell handler
    /// * `msg_context` - The IOPub message context
    pub fn new(
        socket: Socket,
        handler: Arc<Mutex<dyn ShellHandler>>,
        msg_context: Arc<Mutex<Option<JupyterHeader>>>,
    ) -> Self {
        Self {
            socket,
            handler,
            msg_context,
        }
    }

    /// Listens for messages on the stdin socket. This follows a simple loop:
    ///
    /// 1. Wait for
    pub fn listen(&self, input_request_rx: Receiver<ShellInputRequest>) {
        // Listen for input requests from the back end
        loop {
            // Wait for a message (input request) from the back end
            let req = input_request_rx.recv().unwrap();

            if let None = req.originator {
                warn!("No originator for stdin request");
            }

            // Deliver the message to the front end
            let msg = JupyterMessage::create_with_identity(
                req.originator,
                req.request,
                &self.socket.session,
            );
            if let Err(err) = msg.send(&self.socket) {
                warn!("Failed to send message to front end: {}", err);
            }
            trace!("Sent input request to front end, waiting for input reply...");

            // Attempt to read the front end's reply message from the ZeroMQ socket.
            //
            // TODO: This will block until the front end sends an input request,
            // which could be a while and perhaps never if the user cancels the
            // operation, never provides input, etc. We should probably have a
            // timeout here, or some way to cancel the read if another input
            // request arrives.
            let message = match Message::read_from_socket(&self.socket) {
                Ok(m) => m,
                Err(err) => {
                    warn!("Could not read message from stdin socket: {}", err);
                    continue;
                },
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
