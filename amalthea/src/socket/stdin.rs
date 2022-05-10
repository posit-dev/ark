/*
 * stdin.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::language::shell_handler::ShellHandler;
use crate::socket::socket::Socket;
use crate::wire::input_request::InputRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use futures::executor::block_on;
use log::{trace, warn};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::thread;

pub struct Stdin {
    /// The ZeroMQ stdin socket
    socket: Socket,

    /// Language-provided shell handler object
    handler: Arc<Mutex<dyn ShellHandler>>,
}

impl Stdin {
    /// Create a new Stdin socket
    ///
    /// * `socket` - The underlying ZeroMQ socket
    /// * `handler` - The language's shell handler
    pub fn new(socket: Socket, handler: Arc<Mutex<dyn ShellHandler>>) -> Self {
        Self {
            socket: socket,
            handler: handler,
        }
    }

    /// Listens for messages on the stdin socket. This goes two ways: we listen
    /// for input requests from the back end and input replies on the front end.
    pub fn listen(&self) {
        // Create the thread to listen for input requests from the back end.
        let handler = self.handler.clone();
        let socket = self.socket.clone();
        thread::spawn(move || Self::listen_backend(handler, socket));

        // Listen for input replies from the front end
        self.listen_frontend();
    }

    pub fn listen_backend(handler: Arc<Mutex<dyn ShellHandler>>, socket: Socket) {
        // Create the communication channel for the shell handler and inject it
        let (sender, receiver) = sync_channel::<InputRequest>(1);
        {
            let mut shell_handler = handler.lock().unwrap();
            shell_handler.establish_input_handler(sender);
        }

        // Listen for input requests from the back end
        loop {
            // Wait for a message (input request) from the back end
            let req = receiver.recv().unwrap();

            // Deliver the message to the front end
            let msg = JupyterMessage::create(req, None, &socket.session);
            if let Err(err) = msg.send(&socket) {
                warn!("Failed to send message to front end: {}", err);
            }
        }
    }

    pub fn listen_frontend(&self) {
        loop {
            trace!("Waiting for stdin messages");
            // Attempt to read the next message from the ZeroMQ socket
            let message = match Message::read_from_socket(&self.socket) {
                Ok(m) => m,
                Err(err) => {
                    warn!("Could not read message from stdin socket: {}", err);
                    continue;
                }
            };

            // Only input replies are expected on this socket
            let reply = match message {
                Message::InputReply(reply) => reply,
                _ => {
                    warn!("Received unexpected message on stdin socket: {:?}", message);
                    continue;
                }
            };

            // Send the reply to the shell handler
            let handler = self.handler.lock().unwrap();
            if let Err(err) = block_on(handler.handle_input_reply(&reply.content)) {
                warn!("Error handling input reply: {:?}", err);
            }
        }
    }
}
