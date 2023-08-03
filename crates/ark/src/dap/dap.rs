//
// dap.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::sync::{Arc, Mutex};

use amalthea::{comm::comm_channel::CommChannelMsg, language::dap_handler::DapHandler};
use crossbeam::channel::Sender;
use harp::session::FrameInfo;
use serde_json::json;
use stdext::spawn;

use crate::dap::dap_server;

pub struct Dap {
    /// State shared with the DAP server thread.
    pub state: Arc<Mutex<DapState>>,

    /// Channel for sending events to frontend.
    comm_tx: Option<Sender<CommChannelMsg>>,

    /// Whether we are connected to the frontend.
    connected: bool,
}

pub struct DapState {
    /// Whether the REPL is stopped with a browser prompt.
    pub debugging: bool,

    /// Stack information
    pub stack: Option<Vec<FrameInfo>>,
}

impl DapState {
    pub fn new() -> Self {
        Self {
            debugging: false,
            stack: None,
        }
    }
}

impl Dap {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(DapState::new())),
            comm_tx: None,
            connected: false,
        }
    }

    pub fn start_debug(&self, stack: Vec<FrameInfo>) {
        let mut state = self.state.lock().unwrap();

        // TODO: We probably need to send pause events to the server
        state.stack = Some(stack);

        if !state.debugging {
            // FIXME: Should this be a `prompt_debug` event? We are not
            // necessarily starting to debug, we just want to let the frontend
            // know we are running in debug mode on the backend side.
            if let Some(tx) = &self.comm_tx {
                log::info!("DAP: Sending `start_debug` event");
                let msg = CommChannelMsg::Data(json!({
                    "msg_type": "start_debug",
                    "content": {}
                }));
                tx.send(msg).unwrap();
            }

            state.debugging = true;
        }
    }

    pub fn stop_debug(&self) {
        // Reset state
        let mut state = self.state.lock().unwrap();
        *state = DapState::new();
    }
}

// Handler for Amalthea socket threads
impl DapHandler for Dap {
    fn start(
        &mut self,
        tcp_address: String,
        comm_tx: Sender<CommChannelMsg>,
    ) -> Result<(), amalthea::error::Error> {
        log::info!("DAP: Spawning thread");

        // Create the DAP thread that manages connections and creates a
        // server when connected. This is currently the only way to create
        // this thread but in the future we might provide other ways to
        // connect to the DAP without a Jupyter comm.
        let state_clone = self.state.clone();
        spawn!("ark-dap", move || {
            dap_server::start_dap(tcp_address, state_clone)
        });

        // If `start()` is called we are now connected to a frontend
        self.comm_tx = Some(comm_tx);
        self.connected = true;

        return Ok(());
    }
}
