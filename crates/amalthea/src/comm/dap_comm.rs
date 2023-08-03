/*
 * dap_comm.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::Sender;
use serde::Deserialize;
use serde::Serialize;

use crate::comm::comm_channel::CommChannelMsg;
use crate::error::Error;
use crate::language::dap_handler::DapHandler;

#[derive(Debug, Serialize, Deserialize)]
pub struct StartDap {
    /// The address on which the client is listening for DAP requests.
    pub client_address: String,
}

pub struct DapComm {
    handler: Arc<Mutex<dyn DapHandler>>,
    msg_tx: Sender<CommChannelMsg>,
}

/**
 * DapComm makes a DAP look like a CommChannel; it's used to start the DAP and
 * track the server thread.
 *
 * - `handler` is the DAP handler that will be used to start the DAP.
 * - `msg_tx` is the channel that will be used to send messages to the front end.
 */
impl DapComm {
    pub fn new(handler: Arc<Mutex<dyn DapHandler>>, msg_tx: Sender<CommChannelMsg>) -> DapComm {
        DapComm { handler, msg_tx }
    }

    pub fn start(&self, data: &StartDap, conn_init_tx: Sender<bool>) -> Result<(), Error> {
        let mut handler = self.handler.lock().unwrap();
        handler
            .start(
                data.client_address.clone(),
                conn_init_tx,
                self.msg_tx.clone(),
            )
            .unwrap();
        Ok(())
    }

    /**
     * Returns a Sender that can accept comm channel messages (required as
     * part of the `CommChannel` contract). Because the DAP communicates
     * over its own TCP connection, it does not process messages from the
     * comm, and they are discarded here.
     */
    pub fn msg_sender(&self) -> Sender<CommChannelMsg> {
        let (msg_tx, _msg_rx) = crossbeam::channel::unbounded();
        msg_tx
    }
}
