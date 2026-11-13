//
// handler.rs
//
// Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
//
//

use std::path::PathBuf;
use std::sync::Arc;

use amalthea::comm::server_comm::ServerStartMessage;
use amalthea::comm::server_comm::ServerStartedMessage;
use amalthea::language::server_handler::ServerHandler;
use amalthea::socket::comm::CommOutgoingTx;
use anyhow::Context;
use bus::BusReader;
use crossbeam::channel::Sender;
use stdext::result::ResultExt;
use stdext::spawn;
use tokio::net::TcpListener;
use tokio::runtime::Builder;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::UnboundedSender as AsyncUnboundedSender;

use super::backend;
use crate::console::Console;
use crate::console::ConsoleNotification;
use crate::console::KernelInfo;
use crate::lsp::host;
use crate::lsp::host::ArkHandlers;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::panic;
use crate::panic::Recovery;
use crate::r_task;

pub(crate) struct Lsp {
    r_home: PathBuf,
    runtime: Arc<Runtime>,
    kernel_init_rx: BusReader<KernelInfo>,
    kernel_initialized: bool,
    console_notification_tx: AsyncUnboundedSender<ConsoleNotification>,
}

impl Lsp {
    pub(crate) fn new(
        r_home: PathBuf,
        kernel_init_rx: BusReader<KernelInfo>,
        console_notification_tx: AsyncUnboundedSender<ConsoleNotification>,
    ) -> Self {
        let rt = Builder::new_multi_thread()
            .enable_all()
            // Workers serve tower-lsp and the auxiliary loop. The main loop has
            // its own thread, and background analysis, workspace scans, and
            // source fetches have their own pools of OS threads.
            .worker_threads(2)
            .build()
            .unwrap();

        Self {
            r_home,
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
        _comm_tx: CommOutgoingTx,
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

        let r_home = self.r_home.clone();
        let console_notification_tx = self.console_notification_tx.clone();
        spawn!("ark-lsp", move || {
            start_lsp(
                r_home,
                runtime,
                server_start,
                server_started_tx,
                console_notification_tx,
            )
        });
        Ok(())
    }
}

/// Announce a TCP port to the frontend, accept one client, and serve the LSP
/// with Ark's host until the connection ends or the server shuts down.
fn start_lsp(
    r_home: PathBuf,
    runtime: Arc<Runtime>,
    server_start: ServerStartMessage,
    server_started_tx: Sender<ServerStartedMessage>,
    console_notification_tx: AsyncUnboundedSender<ConsoleNotification>,
) {
    let lifecycle = async {
        let ip_address = server_start.ip_address();

        // Binding to port `0` to allow the OS to allocate a port for us to bind to
        let listener = match TcpListener::bind(format!("{ip_address}:0")).await {
            Ok(listener) => listener,
            Err(err) => {
                log::error!("LSP: Failed to bind to {ip_address}:0: {err:?}");
                return;
            },
        };

        let address = match listener.local_addr() {
            Ok(address) => address,
            Err(error) => {
                log::error!("LSP: Failed to get local address for {ip_address}:0: {error}");
                return;
            },
        };

        // Get the OS allocated port
        let port = address.port();

        log::trace!("LSP: Thread starting at address {ip_address}:{port}.");

        // Send the port back to `Shell` and eventually out to the frontend so it can connect
        server_started_tx
            .send(ServerStartedMessage::new(port))
            .context("LSP: Can't send server started notification")
            .log_err();

        log::trace!("LSP: Waiting for client");
        let (stream, address) = match listener.accept().await {
            Ok(connection) => connection,
            Err(err) => {
                log::error!("LSP: Failed to accept a client connection: {err:?}");
                return;
            },
        };
        log::trace!("LSP: Connected to client: '{address}'");
        let (read, write) = tokio::io::split(stream);

        // The embedded session's `.libPaths()` cannot describe every workspace
        // environment. Resolution starts fresh R processes at `initialize`.
        let r = oak_r_process::r_executable(&r_home);
        if r.is_none() {
            log::warn!(
                "Can't locate an R executable under '{r_home}', source fetching, the sidecar job and R library resolution are disabled",
                r_home = r_home.display()
            );
        }

        // Forward event channel along to `Console`.
        // This also updates an outdated channel after a reconnect.
        // `Console` should be initialized by now, since the caller of this
        // function waits to receive the init notification sent on
        // `kernel_init_rx`. Even if it isn't, this should be okay because
        // `r_task()` defensively blocks until its sender is initialized.
        let set_console_channel = |events_tx: &TokioUnboundedSender<Event<ArkHandlers>>| {
            let events_tx = events_tx.clone();
            r_task(move || {
                Console::get_mut().set_lsp_channel(events_tx);
            });
        };

        backend::serve(
            read,
            write,
            ArkHandlers::new(console_notification_tx),
            r,
            set_console_channel,
            host::custom_methods,
        )
        .await;

        log::trace!("LSP: Connection to client '{address}' ended.");

        // Stop console updates to this connection's main loop. `serve()` also
        // returns after a service panic, so cleanup allows the client to reconnect.
        r_task({
            move || {
                Console::get_mut().remove_lsp_channel();
            }
        });
    };

    if let Err(err) = runtime.block_on(panic::catch_unwind_async(Recovery::Always, lifecycle)) {
        log::error!("LSP: Panic in the server lifecycle: {err:?}");
    }
}
