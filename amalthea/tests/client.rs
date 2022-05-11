/*
 * client.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::kernel::Kernel;
use amalthea::socket::iopub::IOPubMessage;
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

    let kernel = Kernel::new(frontend.get_connection_file());
    match kernel {
        Ok(k) => match k.connect(shell, control, iopub_sender, iopub_receiver) {
            Ok(()) => {
                let mut s = String::new();
                println!("Kernel activated, press Ctrl+C to end ");
                if let Err(err) = stdin().read_line(&mut s) {
                    error!("Could not read from stdin: {}", err);
                }
            }
            Err(err) => {
                error!("Couldn't connect to front end: {:?}", err);
            }
        },
        Err(err) => {
            error!("Couldn't create kernel: {:?}", err);
        }
    }
}
