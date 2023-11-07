/*
 * control.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use amalthea::language::control_handler::ControlHandler;
use amalthea::wire::exception::Exception;
use amalthea::wire::interrupt_reply::InterruptReply;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::shutdown_reply::ShutdownReply;
use amalthea::wire::shutdown_request::ShutdownRequest;
use async_trait::async_trait;
use crossbeam::channel::Sender;
use log::*;

use crate::request::RRequest;

pub struct Control {
    r_request_tx: Sender<RRequest>,
}

impl Control {
    pub fn new(sender: Sender<RRequest>) -> Self {
        Self {
            r_request_tx: sender,
        }
    }
}

#[async_trait]
impl ControlHandler for Control {
    async fn handle_shutdown_request(
        &self,
        msg: &ShutdownRequest,
    ) -> Result<ShutdownReply, Exception> {
        debug!("Received shutdown request: {:?}", msg);

        // According to the Jupyter protocol we should block here until the
        // shutdown is complete. However AFAICS ipykernel doesn't wait
        // until complete shutdown before replying and instead just signals
        // a shutdown via a global flag picked up by an event loop.

        let status = if let Err(err) = self.r_request_tx.send(RRequest::Shutdown(msg.restart)) {
            log::error!("Could not deliver shutdown request to execution thread: {err:?}");
            Status::Error
        } else {
            Status::Ok
        };

        Ok(ShutdownReply {
            status,
            restart: msg.restart,
        })
    }

    async fn handle_interrupt_request(&self) -> Result<InterruptReply, Exception> {
        debug!("Received interrupt request");

        #[cfg(not(windows))]
        {
            use nix::sys::signal::Signal;
            use nix::sys::signal::{self};
            use nix::unistd::Pid;
            signal::kill(Pid::this(), Signal::SIGINT).unwrap();
        }
        #[cfg(windows)]
        {
            log::error!("Interrupts are not supported on Windows yet.");
            // TODO: Windows.
            // Look at https://github.com/Detegr/rust-ctrlc for cross platform handling (plus termination handling)
        }

        // TODO: Needs to send a SIGINT to the whole process group so that
        // processes started by R will also be interrupted.

        Ok(InterruptReply { status: Status::Ok })
    }
}
