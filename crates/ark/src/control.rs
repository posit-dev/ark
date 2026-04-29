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
use crossbeam::channel::Sender;
use stdext::result::ResultExt;

use crate::console::SessionMode;
use crate::dap::dap_jupyter_handler::DapJupyterHandler;
use crate::dap::Dap;
use crate::request::RRequest;

pub struct Control {
    r_request_tx: Sender<RRequest>,
    dap: Arc<Mutex<Dap>>,
    dap_handler: Option<DapJupyterHandler>,
}

impl Control {
    pub fn new(
        r_request_tx: Sender<RRequest>,
        dap: Arc<Mutex<Dap>>,
        iopub_tx: Sender<IOPubMessage>,
        session_mode: SessionMode,
    ) -> Self {
        let dap_handler = if matches!(session_mode, SessionMode::Notebook) {
            dap.lock().unwrap().set_iopub_tx(iopub_tx.clone());
            Some(DapJupyterHandler::new(
                dap.clone(),
                r_request_tx.clone(),
                iopub_tx,
            ))
        } else {
            None
        };

        Self {
            r_request_tx,
            dap,
            dap_handler,
        }
    }
}

impl ControlHandler for Control {
    fn handle_shutdown_request(&self, msg: &ShutdownRequest) -> Result<ShutdownReply, Exception> {
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

    fn handle_interrupt_request(&self) -> Result<InterruptReply, Exception> {
        log::info!("Received interrupt request");

        // When an interrupt is sent while debugging in notebook mode, we quit
        // the debugger. The difference is justified by how the Console stays
        // busy while debugging, showing a spinning wheel to the user. Quitting
        // debugging on interrupt is natural UX in that context.
        if self.dap_handler.is_some() {
            let dap = self.dap.lock().unwrap();
            if dap.is_debugging || dap.is_debugging_stdin {
                drop(dap);
                self.r_request_tx
                    .send(RRequest::DebugCommand(crate::request::DebugRequest::Quit))
                    .log_err();
            }
        }
        crate::sys::control::handle_interrupt_request();

        Ok(InterruptReply { status: Status::Ok })
    }

    fn handle_debug_request(&self, msg: &DebugRequest) -> Result<DebugReply, Exception> {
        let Some(handler) = &self.dap_handler else {
            let response = serde_json::json!({
                "seq": 0,
                "type": "response",
                "success": false,
                "message": "Debug requests are not supported in console mode",
            });
            return Ok(DebugReply { content: response });
        };
        let response = handler.handle(&msg.content);
        Ok(DebugReply { content: response })
    }
}
