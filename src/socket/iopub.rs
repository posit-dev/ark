/*
 * iopub.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::socket::signed_socket::SignedSocket;
use crate::socket::socket::Socket;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;
use log::warn;
use std::sync::mpsc::Receiver;

pub struct IOPub {
    socket: SignedSocket,
    state_receiver: Receiver<ExecutionState>,
    state: ExecutionState,
    busy_depth: u32,
}

impl Socket for IOPub {
    fn name() -> String {
        String::from("IOPub")
    }

    fn kind() -> zmq::SocketType {
        zmq::PUB
    }
}

impl IOPub {
    pub fn new(socket: SignedSocket, receiver: Receiver<ExecutionState>) -> Self {
        Self {
            socket: socket,
            state_receiver: receiver,
            state: ExecutionState::Starting,
            busy_depth: 0,
        }
    }

    pub fn listen(&mut self) {
        // Begin by emitting the starting state
        self.emit_state(ExecutionState::Starting);
        loop {
            let state = match self.state_receiver.recv() {
                Ok(s) => s,
                Err(err) => {
                    warn!("Failed to receive kernel execution status: {}", err);

                    // Wait 5s before trying to receive another state update
                    // (avoid log flood if something is wrong with channel)
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    continue;
                }
            };
            match state {
                ExecutionState::Idle => match self.state {
                    ExecutionState::Busy => {
                        if self.busy_depth > 0 {
                            self.busy_depth = self.busy_depth - 1;
                        } else {
                            self.emit_state(state);
                        }
                    }
                    ExecutionState::Starting => {
                        self.emit_state(state);
                    }
                    ExecutionState::Idle => {
                        // Do nothing
                    }
                },
                ExecutionState::Busy => match self.state {
                    ExecutionState::Busy => {
                        self.busy_depth = self.busy_depth + 1;
                    }
                    ExecutionState::Idle | ExecutionState::Starting => {
                        self.emit_state(state);
                    }
                },
                _ => {
                    warn!(
                        "Invalid kernel state transition from {:?} to {:?}",
                        self.state, state
                    )
                }
            }
        }
    }

    fn emit_state(&mut self, state: ExecutionState) {
        self.state = state;
        // TODO the parent header could be the message that sent the kernel into
        // the busy/idle state. do clients care?
        if let Err(err) = JupyterMessage::<KernelStatus>::create(
            KernelStatus {
                execution_state: self.state,
            },
            None,
            &self.socket.session,
        )
        .send(&self.socket)
        {
            warn!("Could not emit kernel's startup status. {}", err)
        }
    }
}
