/*
 * stdin.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::language::shell_handler::ShellHandler;
use crate::socket::socket::Socket;
use crate::wire::jupyter_message::Message;
use futures::executor::block_on;
use log::{trace, warn};
use std::sync::{Arc, Mutex};

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

    /// Listens for messages on the stdin socket (does not return)
    pub fn listen(&self) {
        loop {
            trace!("Waiting for shell messages");
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
