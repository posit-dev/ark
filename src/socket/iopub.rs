/*
 * iopub.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::socket::socket::Socket;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;
use log::{trace, warn};
use std::sync::mpsc::Receiver;

pub struct IOPub {
    socket: Socket,
    receiver: Receiver<Message>,
}

impl IOPub {
    pub fn new(socket: Socket, receiver: Receiver<Message>) -> Self {
        Self {
            socket: socket,
            receiver: receiver,
        }
    }

    pub fn listen(&self) {
        // Begin by emitting the starting state
        self.emit_state(ExecutionState::Starting);
        loop {
            let message = match self.receiver.recv() {
                Ok(m) => m,
                Err(err) => {
                    warn!("Failed to receive iopub message: {}", err);
                    continue;
                }
            };
            if let Err(err) = self.process_message(message) {
                warn!("Error delivering iopub message: {}", err)
            }
        }
    }

    fn process_message(&self, message: Message) -> Result<(), Error> {
        match message {
            Message::Status(msg) => self.send_message(msg),
            Message::ExecuteResult(msg) => self.send_message(msg),
            Message::ExecuteError(msg) => self.send_message(msg),
            _ => Err(Error::UnsupportedMessage(message, String::from("iopub"))),
        }
    }

    fn send_message<T: ProtocolMessage>(&self, message: JupyterMessage<T>) -> Result<(), Error> {
        message.send(&self.socket)
    }

    fn emit_state(&self, state: ExecutionState) {
        trace!("started emitting state");
        if let Err(err) = JupyterMessage::<KernelStatus>::create(
            KernelStatus {
                execution_state: state,
            },
            None,
            &self.socket.session,
        )
        .send(&self.socket)
        {
            warn!("Could not emit kernel's startup status. {}", err)
        }
        trace!("finished emitting state!");
    }
}
