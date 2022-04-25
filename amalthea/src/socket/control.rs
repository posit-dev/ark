/*
 * control.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::socket::socket::Socket;
use crate::wire::jupyter_message::Message;
use log::{info, trace, warn};

pub struct Control {
    socket: Socket,
}

impl Control {
    pub fn new(socket: Socket) -> Self {
        Self { socket: socket }
    }

    /// Main loop for the Control thread; to be invoked by the kernel.
    pub fn listen(&self) {
        loop {
            trace!("Waiting for control messages");
            // Attempt to read the next message from the ZeroMQ socket
            let message = match Message::read_from_socket(&self.socket) {
                Ok(m) => m,
                Err(err) => {
                    warn!("Could not read message from control socket: {}", err);
                    continue;
                }
            };

            match message {
                Message::ShutdownRequest(req) => {
                    info!("Received shutdown request, shutting down kernel: {:?}", req);
                    break;
                }
                _ => warn!(
                    "{}",
                    Error::UnsupportedMessage(message, String::from("Control"))
                ),
            }
        }
    }
}
