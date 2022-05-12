/*
 * client.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::kernel::Kernel;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::jupyter_message::{JupyterMessage, Message};
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use log::{debug, error, info};
use std::io::stdin;
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};

mod control;
mod frontend;
mod shell;

#[test]
fn test_kernel() {
    // This channel delivers execution status and other iopub messages from
    // other threads to the iopub thread
    let (iopub_sender, iopub_receiver) = sync_channel::<IOPubMessage>(10);

    let shell_sender = iopub_sender.clone();
    let shell = Arc::new(Mutex::new(shell::Shell::new(shell_sender)));
    let control = Arc::new(Mutex::new(control::Control {}));
    let frontend = frontend::Frontend::new();

    // Create and connect the kernel to the front end
    let kernel = Kernel::new(frontend.get_connection_file()).unwrap();
    kernel
        .connect(shell, control, iopub_sender, iopub_receiver)
        .unwrap();

    // Ask the kernel for the kernel info
    frontend.send_shell(KernelInfoRequest {});

    let reply = frontend.receive();
    match reply {
        Message::KernelInfoReply(reply) => {
            info!("Kernel info: {:?}", reply);
            assert_eq!(reply.content.language_info.name, "Test");
        }
        _ => {
            panic!("Unexpected message received: {:?}", reply);
        }
    }
}
