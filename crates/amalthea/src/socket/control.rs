/*
 * control.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::Sender;
use futures::executor::block_on;
use log::error;
use log::info;
use log::trace;
use log::warn;

use crate::error::Error;
use crate::language::control_handler::ControlHandler;
use crate::socket::socket::Socket;
use crate::wire::jupyter_message::Message;

pub struct Control {
    socket: Socket,
    handler: Arc<Mutex<dyn ControlHandler>>,
    stdin_interrupt_tx: Sender<bool>,
}

impl Control {
    pub fn new(
        socket: Socket,
        handler: Arc<Mutex<dyn ControlHandler>>,
        stdin_interrupt_tx: Sender<bool>,
    ) -> Self {
        Self {
            socket,
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

            match message {
                Message::ShutdownRequest(req) => {
                    info!("Received shutdown request, shutting down kernel: {:?}", req);

                    // Lock the shell handler object on this thread
                    let shell_handler = self.handler.lock().unwrap();
                    if let Err(err) = block_on(shell_handler.handle_shutdown_request(&req.content))
                    {
                        warn!("Failed to handle shutdown request: {:?}", err);
                        // TODO: if this fails, maybe we need to force a process shutdown?
                    }
                    break;
                },
                Message::InterruptRequest(req) => {
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

                    let control_handler = self.handler.lock().unwrap();
                    if let Err(err) = block_on(control_handler.handle_interrupt_request()) {
                        error!("Failed to handle interrupt request: {:?}", err);
                    }
                    // TODO: What happens if the interrupt isn't handled?
                },
                _ => warn!(
                    "{}",
                    Error::UnsupportedMessage(message, String::from("Control"))
                ),
            }
        }
    }
}
