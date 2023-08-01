//
// dap.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use amalthea::{comm::comm_channel::CommChannelMsg, language::dap_handler::DapHandler};
use crossbeam::channel::Sender;
use serde_json::json;
use stdext::spawn;

use crate::dap::dap_server;

pub struct Dap {
    running: bool,
    comm_tx: Option<Sender<CommChannelMsg>>,
}

impl Dap {
    pub fn new() -> Self {
        Self {
            running: false,
            comm_tx: None,
        }
    }

    pub fn start_debug(&self) {
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
        spawn!("ark-dap", move || { dap_server::start_dap(tcp_address) });

        self.running = true;
        self.comm_tx = Some(comm_tx);
        return Ok(());
    }
}
