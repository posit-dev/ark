/*
 * server_comm.rs
 *
 * Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
 *
 */

use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::Sender;

use crate::error::Error;
use crate::language::server_handler::ServerHandler;
use crate::socket::comm::CommOutgoingTx;

/// Message sent from the frontend requesting a server to start
#[derive(Debug, serde::Deserialize)]
pub struct ServerStartMessage {
    /// The IP address on which the client is listening for server requests. The server is
    /// in charge of picking the exact port to communicate over, since the server is the
    /// one that binds to the port (to prevent race conditions).
    ip_address: String,
}

impl ServerStartMessage {
    pub fn new(ip_address: String) -> Self {
        Self { ip_address }
    }

    pub fn ip_address(&self) -> &str {
        &self.ip_address
    }
}

/// Message sent to the frontend to acknowledge that the corresponding server has started
#[derive(Debug, serde::Serialize)]
pub struct ServerStartedMessage {
    /// The port that the frontend should connect to on the `ip_address` it sent over
    port: u16,
}

impl ServerStartedMessage {
    pub fn new(port: u16) -> Self {
        Self { port }
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

pub struct ServerComm {
    handler: Arc<Mutex<dyn ServerHandler>>,
    msg_tx: CommOutgoingTx,
}

/**
 * ServerComm makes an LSP or DAP object look like a CommChannel; it's used
 * to start the LSP or DAP and track the server thread.
 *
 * - `handler` is the handler that will be used to start the server.
 * - `msg_tx` is the channel that will be used to send messages to the frontend.
 */
impl ServerComm {
    pub fn new(handler: Arc<Mutex<dyn ServerHandler>>, msg_tx: CommOutgoingTx) -> ServerComm {
        ServerComm { handler, msg_tx }
    }

    /// This should return immediately after starting the server in a
    /// separate thread. Signal that the server is ready to accept
    /// connection by sending `true` via `server_started_tx`.
    pub fn start(
        &self,
        server_start: ServerStartMessage,
        server_started_tx: Sender<ServerStartedMessage>,
    ) -> Result<(), Error> {
        let mut handler = self.handler.lock().unwrap();
        handler.start(server_start, server_started_tx, self.msg_tx.clone())?;
        Ok(())
    }
}
