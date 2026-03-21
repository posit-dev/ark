/*
 * control.rs
 *
 * Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
 *
 */

use amalthea::language::control_handler::ControlHandler;
use amalthea::wire::debug_reply::DebugReply;
use amalthea::wire::debug_request::DebugRequest;
use amalthea::wire::exception::Exception;
use amalthea::wire::interrupt_reply::InterruptReply;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::shutdown_reply::ShutdownReply;
use amalthea::wire::shutdown_request::ShutdownRequest;
use async_trait::async_trait;
use crossbeam::channel::Sender;

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
        log::info!("Received debug request: {msg:?}");

        // TODO: Route to the DAP command handling logic.
        // For now, return a DAP error response indicating debugging
        // is not yet supported via the Jupyter debug channel.
        let seq = msg.content.get("seq").and_then(|v| v.as_i64()).unwrap_or(0);
        let command = msg
            .content
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let response = serde_json::json!({
            "seq": 0,
            "type": "response",
            "request_seq": seq,
            "success": false,
            "command": command,
            "message": "Notebook debugging is not yet supported",
        });

        Ok(DebugReply { content: response })
    }
}
