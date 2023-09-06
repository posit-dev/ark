//
// handler.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::comm_channel::CommChannelMsg;
use amalthea::language::server_handler::ServerHandler;
use bus::BusReader;
use crossbeam::channel::Sender;
use stdext::spawn;

use super::backend;
use crate::interface::KernelInfo;
use crate::request::KernelRequest;

pub struct Lsp {
    kernel_request_tx: Sender<KernelRequest>,
    kernel_init_rx: BusReader<KernelInfo>,
    kernel_initialized: bool,
}

impl Lsp {
    pub fn new(
        kernel_request_tx: Sender<KernelRequest>,
        kernel_init_rx: BusReader<KernelInfo>,
    ) -> Self {
        Self {
            kernel_request_tx,
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
        _comm_tx: Sender<CommChannelMsg>,
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

        let kernel_request_tx = self.kernel_request_tx.clone();

        spawn!("ark-lsp", move || {
            backend::start_lsp(tcp_address, kernel_request_tx, conn_init_tx)
        });
        return Ok(());
    }
}
