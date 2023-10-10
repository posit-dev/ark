//
// dap.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Arc;
use std::sync::Mutex;

use amalthea::comm::comm_channel::CommChannelMsg;
use amalthea::language::server_handler::ServerHandler;
use crossbeam::channel::Sender;
use harp::session::FrameInfo;
use serde_json::json;
use stdext::log_error;
use stdext::spawn;

use crate::dap::dap_server;
use crate::request::RRequest;

#[derive(Debug, Copy, Clone)]
pub enum DapBackendEvent {
    /// Event sent when a normal (non-browser) prompt marks the end of a
    /// debugging session.
    Terminated,

    /// Event sent when user types `n`, `f`, `c`, or `cont`.
    Continued,

    /// Event sent when a browser prompt is emitted during an existing
    /// debugging session
    Stopped,
}

pub struct Dap {
    /// Whether the REPL is stopped with a browser prompt.
    pub is_debugging: bool,

    /// Whether the DAP server is connected to a client.
    pub is_connected: bool,

    /// Channel for sending events to the DAP frontend.
    /// This always exists when `is_connected` is true.
    pub backend_events_tx: Option<Sender<DapBackendEvent>>,

    /// Current call stack
    pub stack: Option<Vec<FrameInfo>>,

    /// Channel for sending events to the comm frontend.
    comm_tx: Option<Sender<CommChannelMsg>>,

    /// Channel for sending debug commands to `read_console()`
    r_request_tx: Sender<RRequest>,

    /// Self-reference under a mutex. Shared with the R, Shell socket, and
    /// DAP server threads.
    shared_self: Option<Arc<Mutex<Dap>>>,
}

impl Dap {
    pub fn new_shared(r_request_tx: Sender<RRequest>) -> Arc<Mutex<Self>> {
        let state = Self {
            is_debugging: false,
            is_connected: false,
            backend_events_tx: None,
            stack: None,
            comm_tx: None,
            r_request_tx,
            shared_self: None,
        };

        let shared = Arc::new(Mutex::new(state));

        // Set shareable self-reference
        {
            let mut state = shared.lock().unwrap();
            state.shared_self = Some(shared.clone());
        }

        shared
    }

    pub fn start_debug(&mut self, stack: Vec<FrameInfo>) {
        self.stack = Some(stack);

        if self.is_debugging {
            if let Some(tx) = &self.backend_events_tx {
                log_error!(tx.send(DapBackendEvent::Stopped));
            }
        } else {
            if let Some(tx) = &self.comm_tx {
                // Ask frontend to connect to the DAP
                log::trace!("DAP: Sending `start_debug` event");
                let msg = CommChannelMsg::Data(json!({
                    "msg_type": "start_debug",
                    "content": {}
                }));
                log_error!(tx.send(msg));
            }

            self.is_debugging = true;
        }
    }

    pub fn stop_debug(&mut self) {
        // Reset state
        self.stack = None;
        self.is_debugging = false;

        if self.is_connected {
            if let Some(_) = &self.comm_tx {
                // Let frontend know we've quit the debugger so it can
                // terminate the debugging session and disconnect.
                if let Some(tx) = &self.backend_events_tx {
                    log::trace!("DAP: Sending `stop_debug` event");
                    log_error!(tx.send(DapBackendEvent::Terminated));
                }
            }
            // else: If not connected to a frontend, the DAP client should
            // have received a `Continued` event already, after a `n`
            // command or similar.
        }
    }
}

// Handler for Amalthea socket threads
impl ServerHandler for Dap {
    fn start(
        &mut self,
        tcp_address: String,
        conn_init_tx: Sender<bool>,
        comm_tx: Sender<CommChannelMsg>,
    ) -> Result<(), amalthea::error::Error> {
        log::info!("DAP: Spawning thread");

        // If `start()` is called we are now connected to a frontend
        self.comm_tx = Some(comm_tx.clone());

        // Create the DAP thread that manages connections and creates a
        // server when connected. This is currently the only way to create
        // this thread but in the future we might provide other ways to
        // connect to the DAP without a Jupyter comm.
        let r_request_tx_clone = self.r_request_tx.clone();

        // This can't panic as `Dap` can't be constructed without a shared self
        let state_clone = self.shared_self.as_ref().unwrap().clone();

        spawn!("ark-dap", move || {
            dap_server::start_dap(
                tcp_address,
                state_clone,
                conn_init_tx,
                r_request_tx_clone,
                comm_tx,
            )
        });

        return Ok(());
    }
}
