//
// handler.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::event::CommEvent;
use amalthea::language::lsp_handler::LspHandler;
use bus::BusReader;
use crossbeam::channel::Sender;
use stdext::spawn;

use super::backend;
use crate::interface::KernelInfo;
use crate::request::KernelRequest;

pub struct Lsp {
    kernel_request_tx: Sender<KernelRequest>,
    comm_manager_tx: Sender<CommEvent>,
    kernel_init_rx: BusReader<KernelInfo>,
    kernel_initialized: bool,
}

impl Lsp {
    pub fn new(
        kernel_request_tx: Sender<KernelRequest>,
        comm_manager_tx: Sender<CommEvent>,
        kernel_init_rx: BusReader<KernelInfo>,
    ) -> Self {
        Self {
            kernel_request_tx,
            comm_manager_tx,
            kernel_init_rx,
            kernel_initialized: false,
        }
    }
}

impl LspHandler for Lsp {
    fn start(&mut self, tcp_address: String) -> Result<(), amalthea::error::Error> {
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
        let comm_manager_tx = self.comm_manager_tx.clone();
        spawn!("ark-lsp", move || {
            backend::start_lsp(tcp_address, kernel_request_tx, comm_manager_tx)
        });
        return Ok(());
    }
}
