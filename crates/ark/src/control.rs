/*
 * control.rs
 *
 * Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
 *
 */

use std::sync::Arc;
use std::sync::Mutex;

use amalthea::language::control_handler::ControlHandler;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::debug_reply::DebugReply;
use amalthea::wire::debug_request::DebugRequest;
use amalthea::wire::exception::Exception;
use amalthea::wire::interrupt_reply::InterruptReply;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::shutdown_reply::ShutdownReply;
use amalthea::wire::shutdown_request::ShutdownRequest;
use async_trait::async_trait;
use crossbeam::channel::Sender;

use crate::console::SessionMode;
use crate::dap::dap_jupyter_handler::DapJupyterHandler;
use crate::dap::Dap;
use crate::request::RRequest;

pub struct Control {
    r_request_tx: Sender<RRequest>,
    dap_handler: DapJupyterHandler,
}

impl Control {
    pub fn new(
        r_request_tx: Sender<RRequest>,
        dap: Arc<Mutex<Dap>>,
        iopub_tx: Sender<IOPubMessage>,
        session_mode: SessionMode,
    ) -> Self {
        if matches!(session_mode, SessionMode::Notebook) {
            dap.lock().unwrap().set_iopub_tx(iopub_tx.clone());
        }
        let dap_handler = DapJupyterHandler::new(dap, r_request_tx.clone(), iopub_tx);
        Self {
            r_request_tx,
            dap_handler,
        }
    }
}

#[async_trait]
impl ControlHandler for Control {
    async fn handle_shutdown_request(
        &self,
        msg: &ShutdownRequest,
    ) -> Result<ShutdownReply, Exception> {
        log::info!("Received shutdown request: {msg:?}");

        // Interrupt any ongoing computation. We shut down from ReadConsole when
        // R has become idle again. Note that Positron will have interrupted us
        // beforehand, but another frontend might not have, and it's good to
        // have this as a defensive measure in any case.
        crate::sys::control::handle_interrupt_request();

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
        log::info!("Received interrupt request");
        crate::sys::control::handle_interrupt_request();
        Ok(InterruptReply { status: Status::Ok })
    }

    fn handle_debug_request(&self, msg: &DebugRequest) -> Result<DebugReply, Exception> {
        let response = self.dap_handler.handle(&msg.content);
        Ok(DebugReply { content: response })
    }
}
