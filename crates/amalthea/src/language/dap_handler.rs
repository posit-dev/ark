/*
 * dap_handler.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use async_trait::async_trait;
use crossbeam::channel::Sender;

use crate::{comm::comm_channel::CommChannelMsg, error::Error};

/// A trait for handling DAP requests. Not all kernels will support an embedded
/// DAP that communicates over TCP, so this trait is an optional addition for
/// Amalthea-based kernels.
#[async_trait]
pub trait DapHandler: Send {
    /// Starts the DAP server and binds it to the given TCP address.
    fn start(
        &mut self,
        tcp_address: String,
        conn_init_tx: Sender<bool>,
        comm_tx: Sender<CommChannelMsg>,
    ) -> Result<(), Error>;
}
