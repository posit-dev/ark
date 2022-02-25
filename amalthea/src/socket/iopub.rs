/*
 * iopub.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::socket::socket::Socket;
use crate::wire::execute_error::ExecuteError;
use crate::wire::execute_input::ExecuteInput;
use crate::wire::execute_result::ExecuteResult;
use crate::wire::header::JupyterHeader;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;
use log::{trace, warn};
use std::sync::mpsc::Receiver;

pub struct IOPub {
    /// The underlying IOPub socket
    socket: Socket,

    /// A channel that receives IOPub messages from other threads
    receiver: Receiver<IOPubMessage>,

    /// The current message context; attached to outgoing messages to pair
    /// outputs with the message that caused them.
    context: Option<JupyterHeader>,
}

pub enum IOPubMessage {
    Status(JupyterHeader, KernelStatus),
    ExecuteResult(ExecuteResult),
    ExecuteError(ExecuteError),
    ExecuteInput(ExecuteInput),
}

impl IOPub {
    pub fn new(socket: Socket, receiver: Receiver<IOPubMessage>) -> Self {
        Self {
            socket: socket,
            receiver: receiver,
            context: None,
        }
    }

    pub fn listen(&mut self) {
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

    fn process_message(&mut self, message: IOPubMessage) -> Result<(), Error> {
        match message {
            IOPubMessage::Status(context, msg) => {
                self.context = Some(context);
                self.send_message(msg)
            }
            IOPubMessage::ExecuteResult(msg) => self.send_message(msg),
            IOPubMessage::ExecuteError(msg) => self.send_message(msg),
            IOPubMessage::ExecuteInput(msg) => self.send_message(msg),
        }
    }

    fn send_message<T: ProtocolMessage>(&self, content: T) -> Result<(), Error> {
        let msg = JupyterMessage::<T>::create(content, self.context.clone(), &self.socket.session);
        msg.send(&self.socket)
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
