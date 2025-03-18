/*
 * server_handler.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use async_trait::async_trait;
use crossbeam::channel::Sender;

use crate::comm::comm_channel::CommMsg;
use crate::comm::server_comm::ServerStartMessage;
use crate::comm::server_comm::ServerStartedMessage;
use crate::error::Error;

/// A trait for handling LSP and DAP requests. Not all kernels will support
/// these embedded servers that communicates over TCP, so this trait is an
/// optional addition for Amalthea-based kernels.
#[async_trait]
pub trait ServerHandler: Send {
    /// Starts the server using [ServerStartMessage] and sends back a
    /// [ServerStartedMessage]
    fn start(
        &mut self,
        server_start: ServerStartMessage,
        server_started_tx: Sender<ServerStartedMessage>,
        comm_tx: Sender<CommMsg>,
    ) -> Result<(), Error>;
}
