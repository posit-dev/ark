//
// start.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Arc;
use std::sync::Mutex;

use amalthea::comm::event::CommManagerEvent;
use amalthea::connection_file::ConnectionFile;
use amalthea::kernel;
use amalthea::registration_file::RegistrationFile;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::socket::stdin::StdInRequest;
use bus::Bus;
use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;

use crate::control::Control;
use crate::dap;
use crate::interface::SessionMode;
use crate::lsp;
use crate::request::KernelRequest;
use crate::request::RRequest;
use crate::shell::Shell;

/// Exported for unit tests.
pub fn start_kernel(
    connection_file: ConnectionFile,
    registration_file: Option<RegistrationFile>,
    r_args: Vec<String>,
    startup_file: Option<String>,
    session_mode: SessionMode,
    capture_streams: bool,
) {
    // Create the channels used for communication. These are created here
    // as they need to be shared across different components / threads.
    let (iopub_tx, iopub_rx) = bounded::<IOPubMessage>(10);

    // Create the pair of channels that will be used to relay messages from
    // the open comms
    let (comm_manager_tx, comm_manager_rx) = bounded::<CommManagerEvent>(10);

    // A broadcast channel (bus) used to notify clients when the kernel
    // has finished initialization.
    let mut kernel_init_tx = Bus::new(1);

    // A channel pair used for shell requests.
    // These events are used to manage the runtime state, and also to
    // handle message delivery, among other things.
    let (r_request_tx, r_request_rx) = bounded::<RRequest>(1);
    let (kernel_request_tx, kernel_request_rx) = bounded::<KernelRequest>(1);

    // Create the LSP and DAP clients.
    // Not all Amalthea kernels provide these, but ark does.
    // They must be able to deliver messages to the shell channel directly.
    let lsp = Arc::new(Mutex::new(lsp::handler::Lsp::new(kernel_init_tx.add_rx())));

    // DAP needs the `RRequest` channel to communicate with
    // `read_console()` and send commands to the debug interpreter
    let dap = dap::Dap::new_shared(r_request_tx.clone());

    // Communication channel between the R main thread and the Amalthea
    // StdIn socket thread
    let (stdin_request_tx, stdin_request_rx) = bounded::<StdInRequest>(1);

    // Create the shell.
    let kernel_init_rx = kernel_init_tx.add_rx();
    let shell = Shell::new(
        comm_manager_tx.clone(),
        iopub_tx.clone(),
        r_request_tx.clone(),
        stdin_request_tx.clone(),
        kernel_init_rx,
        kernel_request_tx,
        kernel_request_rx,
        session_mode.clone(),
    );

    // Create the control handler; this is used to handle shutdown/interrupt and
    // related requests
    let control = Arc::new(Mutex::new(Control::new(r_request_tx.clone())));

    // Create the stream behavior; this determines whether the kernel should
    // capture stdout/stderr and send them to the frontend as IOPub messages
    let stream_behavior = match capture_streams {
        true => amalthea::kernel::StreamBehavior::Capture,
        false => amalthea::kernel::StreamBehavior::None,
    };

    // Create the Ark kernel
    // TODO: Move the Ark kernel to `RMain`
    let kernel_clone = shell.kernel.clone();
    let shell = Arc::new(Mutex::new(shell));

    let (stdin_reply_tx, stdin_reply_rx) = unbounded();

    let res = kernel::connect(
        "ark",
        connection_file,
        registration_file,
        shell,
        control,
        Some(lsp),
        Some(dap.clone()),
        stream_behavior,
        iopub_tx.clone(),
        iopub_rx,
        comm_manager_tx.clone(),
        comm_manager_rx,
        stdin_request_rx,
        stdin_reply_tx,
    );
    if let Err(err) = res {
        panic!("Couldn't connect to frontend: {err:?}");
    }

    // Start R
    crate::interface::RMain::start(
        r_args,
        startup_file,
        kernel_clone,
        comm_manager_tx,
        r_request_rx,
        stdin_request_tx,
        stdin_reply_rx,
        iopub_tx,
        kernel_init_tx,
        dap,
        session_mode,
    )
}
