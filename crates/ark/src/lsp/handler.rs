//
// handler.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Arc;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::server_comm::ServerStartMessage;
use amalthea::comm::server_comm::ServerStartedMessage;
use amalthea::language::server_handler::ServerHandler;
use bus::BusReader;
use crossbeam::channel::Sender;
use stdext::spawn;
use tokio::runtime::Builder;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedSender as AsyncUnboundedSender;

use super::backend;
use crate::console::ConsoleNotification;
use crate::console::KernelInfo;

pub struct Lsp {
    runtime: Arc<Runtime>,
    kernel_init_rx: BusReader<KernelInfo>,
    kernel_initialized: bool,
    console_notification_tx: AsyncUnboundedSender<ConsoleNotification>,
}

impl Lsp {
    pub fn new(
        kernel_init_rx: BusReader<KernelInfo>,
        console_notification_tx: AsyncUnboundedSender<ConsoleNotification>,
    ) -> Self {
        let rt = Builder::new_multi_thread()
            .enable_all()
            // One for the main loop and one spare
            .worker_threads(2)
            // Used for diagnostics
            .max_blocking_threads(2)
            .build()
            .unwrap();

        Self {
            runtime: Arc::new(rt),
            kernel_init_rx,
            kernel_initialized: false,
            console_notification_tx,
        }
    }
}

impl ServerHandler for Lsp {
    fn start(
        &mut self,
        server_start: ServerStartMessage,
        server_started_tx: Sender<ServerStartedMessage>,
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

        let console_notification_tx = self.console_notification_tx.clone();
        spawn!("ark-lsp", move || {
            backend::start_lsp(
                runtime,
                server_start,
                server_started_tx,
                console_notification_tx,
            )
        });
        return Ok(());
    }
}
