//
// dap_jupyter_handler.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

// Handles DAP requests arriving via Jupyter `debug_request` messages on the
// control channel. Delegates standard DAP commands to the shared `DapHandler`
// and handles Jupyter Debug Protocol extensions (`dumpCell`, `debugInfo`,
// `configurationDone`) directly.
//
// Events are forwarded to the frontend as `debug_event` IOPub messages rather
// than over the TCP stream.
//
// https://jupyter-client.readthedocs.io/en/latest/messaging.html#additions-to-the-dap

use std::cell::Cell;
use std::sync::Arc;
use std::sync::Mutex;

use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::debug_event::DebugEvent;
use crossbeam::channel::Sender;
use dap::base_message::BaseMessage;
use dap::base_message::Sendable;
use dap::requests::Request;
use stdext::result::ResultExt;

use crate::dap::dap_notebook;
use crate::dap::dap_server::DapConsoleEvent;
use crate::dap::dap_server::DapHandler;
use crate::dap::dap_state::Breakpoint;
use crate::dap::dap_state::BreakpointState;
use crate::dap::dap_state::Dap;
use crate::dap::dap_state::THREAD_ID;
use crate::request::RRequest;

pub struct DapJupyterHandler {
    handler: DapHandler,
    iopub_tx: Sender<IOPubMessage>,
    seq: Cell<i64>,
    tmp_file_prefix: &'static str,
}

impl DapJupyterHandler {
    pub fn new(
        state: Arc<Mutex<Dap>>,
        r_request_tx: Sender<RRequest>,
        iopub_tx: Sender<IOPubMessage>,
    ) -> Self {
        let handler = DapHandler::new(state, r_request_tx);
        let tmp_file_prefix = dap_notebook::tmp_file_prefix();

        Self {
            handler,
            iopub_tx,
            seq: Cell::new(1),
            tmp_file_prefix,
        }
    }

    fn next_seq(&self) -> i64 {
        let seq = self.seq.get();
        self.seq.set(seq + 1);
        seq
    }

    /// Process a DAP request from a Jupyter `debug_request` message.
    /// Returns the DAP response to be sent back as a `debug_reply`.
    pub fn handle(&self, request: &serde_json::Value) -> serde_json::Value {
        let seq = request.get("seq").and_then(|v| v.as_i64()).unwrap_or(0);
        let command = request
            .get("command")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        log::trace!("Jupyter DAP: Handling `{command}` (seq={seq})");

        // Handle Jupyter Debug Protocol extensions and commands not in the
        // `dap` crate's `Command` enum.
        let result = match command {
            "dumpCell" => Some(self.handle_dump_cell(seq, request)),
            "debugInfo" => Some(self.handle_debug_info(seq)),
            "configurationDone" => Some(Ok(self.success_response(
                seq,
                "configurationDone",
                serde_json::json!({}),
            ))),
            _ => None,
        };

        match result {
            Some(Ok(response)) => return response,
            Some(Err(err)) => return self.error_response(seq, command, &format!("{err}")),
            None => {},
        }

        // Parse as a standard DAP request and delegate to the shared handler
        match serde_json::from_value::<Request>(request.clone()) {
            Ok(dap_request) => {
                let output = self.handler.dispatch(dap_request);

                for event in output.dap_events {
                    self.send_dap_event(event);
                }

                // Deliver console events directly to R (no detour through the
                // frontend in notebook mode since there is no console prompt to
                // sync)
                for effect in output.console_events {
                    self.handle_console_event(effect);
                }

                self.response_to_json(output.response)
            },
            Err(err) => {
                log::warn!("Jupyter DAP: Failed to parse `{command}`: {err:?}");
                self.error_response(seq, command, &format!("Failed to parse request: {err}"))
            },
        }
    }

    fn handle_console_event(&self, event: DapConsoleEvent) {
        match event {
            DapConsoleEvent::DebugCommand(cmd) => {
                self.handler
                    .r_request_tx
                    .send(RRequest::DebugCommand(cmd))
                    .log_err();
            },
            DapConsoleEvent::Interrupt => {
                crate::sys::control::handle_interrupt_request();
            },
            DapConsoleEvent::Restart => {
                log::warn!("Jupyter DAP: Restart requested but not supported");
            },
        }
    }
}

// Jupyter Debug Protocol extensions
impl DapJupyterHandler {
    /// Receive cell source code and write it to a temporary file so the
    /// debugger can set breakpoints in it.
    ///
    /// https://jupyter-client.readthedocs.io/en/latest/messaging.html#dumpcell
    fn handle_dump_cell(
        &self,
        seq: i64,
        request: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let code = request
            .get("arguments")
            .and_then(|a| a.get("code"))
            .and_then(|c| c.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing `code` in dumpCell arguments"))?;

        let source_path = dap_notebook::notebook_source_path(code);

        std::fs::create_dir_all(self.tmp_file_prefix)?;
        std::fs::write(&source_path, code)?;

        log::trace!("Jupyter DAP: Dumped cell to {source_path}");

        Ok(self.success_response(
            seq,
            "dumpCell",
            serde_json::json!({ "sourcePath": source_path }),
        ))
    }

    /// Return debug state so the frontend can restore breakpoints and configure
    /// source mapping after (re)connecting to the kernel.
    ///
    /// https://jupyter-client.readthedocs.io/en/latest/messaging.html#debuginfo
    fn handle_debug_info(&self, seq: i64) -> anyhow::Result<serde_json::Value> {
        let state = self.handler.state.lock().unwrap();

        let stopped_threads: Vec<i64> = if state.is_debugging {
            vec![THREAD_ID]
        } else {
            vec![]
        };

        let breakpoints: Vec<serde_json::Value> = state
            .breakpoints
            .iter()
            .map(|(uri, (_, bps))| {
                let source = uri
                    .as_url()
                    .to_file_path()
                    .map_or_else(|_| uri.to_string(), |p| p.to_string_lossy().into_owned());
                let source_breakpoints: Vec<serde_json::Value> = bps
                    .iter()
                    .filter(|bp| !matches!(bp.state, BreakpointState::Disabled))
                    .map(|bp| {
                        let mut obj = serde_json::json!({
                            "line": Breakpoint::to_dap_line(bp.original_line),
                        });
                        if let Some(cond) = &bp.condition {
                            obj["condition"] = serde_json::json!(cond);
                        }
                        if let Some(msg) = &bp.log_message {
                            obj["logMessage"] = serde_json::json!(msg);
                        }
                        if let Some(hit) = &bp.hit_condition {
                            obj["hitCondition"] = serde_json::json!(hit);
                        }
                        obj
                    })
                    .collect();
                serde_json::json!({
                    "source": source,
                    "breakpoints": source_breakpoints,
                })
            })
            .collect();

        Ok(self.success_response(
            seq,
            "debugInfo",
            serde_json::json!({
                "isStarted": true,
                "hashMethod": "Murmur2",
                "hashSeed": dap_notebook::hash_seed(),
                "tmpFilePrefix": self.tmp_file_prefix,
                "tmpFileSuffix": dap_notebook::tmp_file_suffix(),
                "breakpoints": breakpoints,
                "stoppedThreads": stopped_threads,
                "richRendering": false,
                "exceptionPaths": [],
            }),
        ))
    }
}

// Serialization helpers
impl DapJupyterHandler {
    fn send_dap_event(&self, event: dap::events::Event) {
        let msg = BaseMessage {
            seq: self.next_seq(),
            message: Sendable::Event(event),
        };

        let json = match serde_json::to_value(&msg) {
            Ok(json) => json,
            Err(err) => {
                log::error!("Jupyter DAP: Failed to serialize event: {err:?}");
                return;
            },
        };

        self.iopub_tx
            .send(IOPubMessage::DebugEvent(DebugEvent { content: json }))
            .log_err();
    }

    fn response_to_json(&self, response: dap::responses::Response) -> serde_json::Value {
        let msg = BaseMessage {
            seq: self.next_seq(),
            message: Sendable::Response(response),
        };

        match serde_json::to_value(&msg) {
            Ok(json) => json,
            Err(err) => {
                log::error!("Jupyter DAP: Failed to serialize response: {err:?}");
                serde_json::json!({
                    "seq": self.next_seq(),
                    "type": "response",
                    "success": false,
                    "message": format!("Internal serialization error: {err}"),
                })
            },
        }
    }

    fn success_response(
        &self,
        request_seq: i64,
        command: &str,
        body: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "seq": self.next_seq(),
            "type": "response",
            "request_seq": request_seq,
            "success": true,
            "command": command,
            "body": body,
        })
    }

    fn error_response(&self, request_seq: i64, command: &str, message: &str) -> serde_json::Value {
        log::warn!("Jupyter DAP: Error for `{command}`: {message}");
        serde_json::json!({
            "seq": self.next_seq(),
            "type": "response",
            "request_seq": request_seq,
            "success": false,
            "command": command,
            "message": message,
        })
    }
}
