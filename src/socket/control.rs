/*
 * control.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::socket::socket::Socket;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::shutdown_request::ShutdownRequest;
use log::{trace, warn};

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

            // Handle the message
            if let Err(err) = self.process_message(message) {
                warn!("Could not process control message: {}", err);
            }
        }
    }

    /// Process a Jupyter message on the control socket.
    fn process_message(&self, msg: Message) -> Result<(), Error> {
        match msg {
            Message::ShutdownRequest(msg) => self.handle_shutdown(msg),
            _ => Err(Error::UnsupportedMessage(msg, String::from("Control"))),
        }
    }

    fn handle_shutdown(&self, msg: JupyterMessage<ShutdownRequest>) -> Result<(), Error> {
        warn!("Kernel shutdown not implemented: {:?}", msg);
        Ok(())
    }
}
