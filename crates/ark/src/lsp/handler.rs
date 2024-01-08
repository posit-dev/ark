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
use tower_lsp::ClientSocket;
use tower_lsp::LspService;

use super::backend;
use crate::interface::KernelInfo;
use crate::lsp::backend::Backend;

pub struct Lsp {
    runtime: Option<Arc<Runtime>>,
    service: Option<LspService<Backend>>,
    socket: Option<ClientSocket>,
    kernel_init_rx: BusReader<KernelInfo>,
    kernel_initialized: bool,
}

impl Lsp {
    pub fn new(
        runtime: Arc<Runtime>,
        service: LspService<Backend>,
        socket: ClientSocket,
        kernel_init_rx: BusReader<KernelInfo>,
    ) -> Self {
        let runtime = Some(runtime);
        let service = Some(service);
        let socket = Some(socket);

        Self {
            runtime,
            service,
            socket,
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

        // Transfer field ownership to the thread
        let runtime = self.runtime.take().unwrap();
        let service = self.service.take().unwrap();
        let socket = self.socket.take().unwrap();

        spawn!("ark-lsp", move || {
            backend::start_lsp(runtime, service, socket, tcp_address, conn_init_tx)
        });
        return Ok(());
    }
}
