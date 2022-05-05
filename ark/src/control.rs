/*
 * control.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::r_request::RRequest;
use amalthea::language::control_handler::ControlHandler;
use amalthea::wire::exception::Exception;
use amalthea::wire::interrupt_reply::InterruptReply;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::shutdown_reply::ShutdownReply;
use amalthea::wire::shutdown_request::ShutdownRequest;
use async_trait::async_trait;
use log::{debug, warn};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use std::sync::mpsc::SyncSender;

pub struct Control {
    req_sender: SyncSender<RRequest>,
}

impl Control {
    pub fn new(sender: SyncSender<RRequest>) -> Self {
        Self { req_sender: sender }
    }
}

#[async_trait]
impl ControlHandler for Control {
    async fn handle_shutdown_request(
        &self,
        msg: &ShutdownRequest,
    ) -> Result<ShutdownReply, Exception> {
        debug!("Received shutdown request: {:?}", msg);
        if let Err(err) = self.req_sender.send(RRequest::Shutdown(msg.restart)) {
            warn!(
                "Could not deliver shutdown request to execution thread: {}",
                err
            )
        }
        Ok(ShutdownReply {
            restart: msg.restart,
        })
    }

    async fn handle_interrupt_request(&self) -> Result<InterruptReply, Exception> {
        debug!("Received interrupt request");
        signal::kill(Pid::this(), Signal::SIGINT).unwrap();
        // TODO: Windows.
        // TODO: Needs to send a SIGINT to the whole process group so that
        // processes started by R will also be interrupted.
        Ok(InterruptReply { status: Status::Ok })
    }
}
