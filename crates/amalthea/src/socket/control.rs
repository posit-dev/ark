/*
 * control.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::SendError;
use crossbeam::channel::Sender;
use futures::executor::block_on;
use log::error;
use log::info;
use log::trace;
use log::warn;
use stdext::unwrap;

use crate::error::Error;
use crate::language::control_handler::ControlHandler;
use crate::socket::iopub::IOPubContextChannel;
use crate::socket::iopub::IOPubMessage;
use crate::socket::socket::Socket;
use crate::wire::interrupt_request::InterruptRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::shutdown_request::ShutdownRequest;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;

pub struct Control {
    socket: Socket,
    iopub_tx: Sender<IOPubMessage>,
    handler: Arc<Mutex<dyn ControlHandler>>,
    stdin_interrupt_tx: Sender<bool>,
}

impl Control {
    pub fn new(
        socket: Socket,
        iopub_tx: Sender<IOPubMessage>,
        handler: Arc<Mutex<dyn ControlHandler>>,
        stdin_interrupt_tx: Sender<bool>,
    ) -> Self {
        Self {
            socket,
            iopub_tx,
            handler,
            stdin_interrupt_tx,
        }
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
                },
            };

            if let Err(err) = self.process_message(message) {
                warn!("Could not handle control message: {err}");
            }
        }
    }

    fn process_message(&self, message: Message) -> Result<(), Error> {
        match message {
            Message::ShutdownRequest(req) => {
                self.handle_request(req, |r| self.handle_shutdown_request(r))
            },
            Message::InterruptRequest(req) => {
                self.handle_request(req, |r| self.handle_interrupt_request(r))
            },
            _ => Err(Error::UnsupportedMessage(message, String::from("control"))),
        }
    }

    /// Sets the kernel state by sending a message on the IOPub channel.
    fn send_state<T: ProtocolMessage>(
        &self,
        parent: JupyterMessage<T>,
        state: ExecutionState,
    ) -> Result<(), SendError<IOPubMessage>> {
        let reply = KernelStatus {
            execution_state: state,
        };
        let message = IOPubMessage::Status(parent.header, IOPubContextChannel::Control, reply);
        self.iopub_tx.send(message)
    }

    fn handle_request<T, H>(&self, req: JupyterMessage<T>, handler: H) -> Result<(), Error>
    where
        T: ProtocolMessage,
        H: FnOnce(JupyterMessage<T>) -> Result<(), Error>,
    {
        // Enter the kernel-busy state in preparation for handling the message.
        if let Err(err) = self.send_state(req.clone(), ExecutionState::Busy) {
            warn!("Failed to change kernel status to busy: {err}");
        }

        // Call amalthea side of the handler.
        let result = handler(req.clone());

        // Return to idle -- we always do this, even if the message generated an
        // error, since many frontends won't submit additional messages until
        // the kernel is marked idle.
        if let Err(err) = self.send_state(req, ExecutionState::Idle) {
            warn!("Failed to restore kernel status to idle: {err}");
        }

        return result;
    }

    fn handle_shutdown_request(&self, req: JupyterMessage<ShutdownRequest>) -> Result<(), Error> {
        info!("Received shutdown request, shutting down kernel: {:?}", req);

        // Lock the control handler object on this thread
        let control_handler = self.handler.lock().unwrap();

        let reply = unwrap!(
            block_on(control_handler.handle_shutdown_request(&req.content)),
            Err(err) => {
                log::error!("Failed to handle shutdown request: {err:?}");
                return Ok(())
                // TODO: if this fails, maybe we need to force a process shutdown?
            }
        );

        // TODO: This currently races with the R thread exiting the
        // REPL and calling `exit()`. Odds are that this is never sent.
        // We should send this from the `R_CleanUp()` frontend method
        // once we implement it.
        unwrap!(
            req.send_reply(reply, &self.socket),
            Err(err) => {
                log::error!("Failed to reply to interrupt request: {err:?}");
            }
        );

        Ok(())
    }

    fn handle_interrupt_request(&self, req: JupyterMessage<InterruptRequest>) -> Result<(), Error> {
        info!(
            "Received interrupt request, asking kernel to stop: {:?}",
            req
        );

        // Notify StdIn socket first in case it's waiting for
        // input which is never going to come because of the
        // interrupt
        if let Err(err) = self.stdin_interrupt_tx.send(true) {
            error!("Failed to send interrupt request: {:?}", err);
        }

        // Lock the control handler object on this thread
        let control_handler = self.handler.lock().unwrap();

        let reply = unwrap!(
            block_on(control_handler.handle_interrupt_request()),
            Err(err) => {
                log::error!("Failed to handle interrupt request: {err:?}");
                return Ok(())
                // TODO: What happens if the interrupt isn't handled?
            }
        );

        unwrap!(
            req.send_reply(reply, &self.socket),
            Err(err) => {
                log::error!("Failed to reply to interrupt request: {err:?}");
            }
        );

        Ok(())
    }
}
