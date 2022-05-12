/*
 * client.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::kernel::Kernel;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use env_logger;
use log::info;
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::thread;

mod control;
mod frontend;
mod shell;

// One-time construction of the logging system.
use std::sync::Once;
static INIT: Once = Once::new();
fn setup() {
    INIT.call_once(|| {
        env_logger::init();
    });
}

#[test]
fn test_kernel() {
    // Set up test logger
    setup();

    // This channel delivers execution status and other iopub messages from
    // other threads to the iopub thread
    let (iopub_sender, iopub_receiver) = sync_channel::<IOPubMessage>(10);

    let shell_sender = iopub_sender.clone();
    let shell = Arc::new(Mutex::new(shell::Shell::new(shell_sender)));
    let control = Arc::new(Mutex::new(control::Control {}));
    let frontend = frontend::Frontend::new();
    let connection_file = frontend.get_connection_file();

    // Create the thread that will run the Amalthea kernel
    thread::spawn(move || {
        let kernel = Kernel::new(connection_file).unwrap();
        kernel
            .connect(shell, control, iopub_sender, iopub_receiver)
            .unwrap();
    });

    // Ask the kernel for the kernel info
    frontend.send_shell(KernelInfoRequest {});

    let reply = frontend.receive_shell();
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
