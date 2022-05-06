/*
 * stdin.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::socket::socket::Socket;
use crate::wire::jupyter_message::Message;
use log::{trace, warn};

pub struct Stdin {
    /// The ZeroMQ stdin socket
    socket: Socket,
}

impl Stdin {
    /// Create a new Stdin socket
    ///
    /// * `socket` - The underlying ZeroMQ socket
    pub fn new(socket: Socket) -> Self {
        Self { socket }
    }

    /// Listens for messages on the stdin socket (does not return)
    pub fn listen(&self) {
        loop {
            trace!("Waiting for shell messages");
            // Attempt to read the next message from the ZeroMQ socket
            let _message = match Message::read_from_socket(&self.socket) {
                Ok(m) => m,
                Err(err) => {
                    warn!("Could not read message from stdin socket: {}", err);
                    continue;
                }
            };

            // TODO: message should probably be handled by the shell handler
        }
    }
}
