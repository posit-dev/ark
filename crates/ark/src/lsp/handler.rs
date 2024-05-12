//
// handler.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Arc;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::language::server_handler::ServerHandler;
use bus::BusReader;
use crossbeam::channel::Sender;
use stdext::spawn;
use tokio::runtime::Runtime;

use super::backend;
use crate::interface::KernelInfo;

pub struct Lsp {
    runtime: Arc<Runtime>,
    kernel_init_rx: BusReader<KernelInfo>,
    kernel_initialized: bool,
}

impl Lsp {
    pub fn new(kernel_init_rx: BusReader<KernelInfo>) -> Self {
        Self {
            runtime: Arc::new(tokio::runtime::Runtime::new().unwrap()),
            kernel_init_rx,
            kernel_initialized: false,
        }
    }
}

impl ServerHandler for Lsp {
    fn start(
        &mut self,
        tcp_address: String,
        conn_init_tx: Sender<bool>,
        _comm_tx: Sender<CommMsg>,
    ) -> Result<(), amalthea::error::Error> {
        // If the kernel hasn't been initialized yet, wait for it to finish.
        // This prevents the LSP from attempting to start up before the kernel
        // is ready; on subsequent starts (reconnects), the kernel will already
        // be initialized.
        if !self.kernel_initialized {
            let status = self.kernel_init_rx.recv();
            if let Err(error) = status {
                log::error!("Error waiting for kernel to initialize: {}", error);
            }
            self.kernel_initialized = true;
        }

        // Retain ownership of the tokio `runtime` inside the `Lsp` to
        // account for potential reconnects
        let runtime = self.runtime.clone();

        spawn!("ark-lsp", move || {
            backend::start_lsp(runtime, tcp_address, conn_init_tx)
        });
        return Ok(());
    }
}
