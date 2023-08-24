/*
 * server_comm.rs
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
use crate::language::server_handler::ServerHandler;

#[derive(Debug, Serialize, Deserialize)]
pub struct StartServer {
    /// The address on which the client is listening for server requests.
    pub client_address: String,
}

pub struct ServerComm {
    handler: Arc<Mutex<dyn ServerHandler>>,
    msg_tx: Sender<CommChannelMsg>,
}

/**
 * ServerComm makes an LSP or DAP object look like a CommChannel; it's used
 * to start the LSP or DAP and track the server thread.
 *
 * - `handler` is the handler that will be used to start the server.
 * - `msg_tx` is the channel that will be used to send messages to the front end.
 */
impl ServerComm {
    pub fn new(
        handler: Arc<Mutex<dyn ServerHandler>>,
        msg_tx: Sender<CommChannelMsg>,
    ) -> ServerComm {
        ServerComm { handler, msg_tx }
    }

    /// This should return immediately after starting the server in a
    /// separate thread. Signal that the server is ready to accept
    /// connection by sending `true` via `conn_init_tx`.
    pub fn start(&self, data: StartServer, conn_init_tx: Sender<bool>) -> Result<(), Error> {
        let mut handler = self.handler.lock().unwrap();
        handler.start(
            data.client_address.clone(),
            conn_init_tx,
            self.msg_tx.clone(),
        )?;
        Ok(())
    }

    /**
     * Returns a Sender that can accept comm channel messages (required as
     * part of the `CommChannel` contract). Because the LSP or DAP
     * communicate over their own TCP connection, they do not process
     * messages from the comm, and they are discarded here.
     */
    pub fn msg_sender(&self) -> Sender<CommChannelMsg> {
        let (msg_tx, _msg_rx) = crossbeam::channel::unbounded();
        msg_tx
    }
}
