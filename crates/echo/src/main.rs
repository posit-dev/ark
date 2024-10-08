/*
 * main.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

mod control;
mod shell;

use std::env;
use std::io::stdin;
use std::sync::Arc;
use std::sync::Mutex;

use amalthea::comm::event::CommManagerEvent;
use amalthea::connection_file::ConnectionFile;
use amalthea::kernel;
use amalthea::kernel::StreamBehavior;
use amalthea::kernel_spec::KernelSpec;
use amalthea::registration_file::RegistrationFile;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::socket::stdin::StdInRequest;
use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;

use crate::control::Control;
use crate::shell::Shell;

fn start_kernel(connection_file: ConnectionFile, registration_file: Option<RegistrationFile>) {
    let (iopub_tx, iopub_rx) = bounded::<IOPubMessage>(10);

    let (comm_manager_tx, comm_manager_rx) = bounded::<CommManagerEvent>(10);

    // Communication channel with StdIn
    let (stdin_request_tx, stdin_request_rx) = bounded::<StdInRequest>(1);
    let (stdin_reply_tx, stdin_reply_rx) = unbounded();

    let shell = Arc::new(Mutex::new(Shell::new(
        iopub_tx.clone(),
        stdin_request_tx,
        stdin_reply_rx,
    )));
    let control = Arc::new(Mutex::new(Control {}));

    if let Err(err) = kernel::connect(
        "echo",
        connection_file,
        registration_file,
        shell,
        control,
        None,
        None,
        StreamBehavior::None,
        iopub_tx,
        iopub_rx,
        comm_manager_tx,
        comm_manager_rx,
        stdin_request_rx,
        stdin_reply_tx,
    ) {
        panic!("Couldn't connect to frontend: {err:?}");
    }

    let mut s = String::new();
    println!("Kernel activated, press Ctrl+C to end ");
    if let Err(err) = stdin().read_line(&mut s) {
        log::error!("Could not read from stdin: {err:?}");
    }

    // FIXME: This currently returns immediately.
    // Should block on message from Control thread instead.
}

fn install_kernel_spec() {
    match env::current_exe() {
        Ok(exe_path) => {
            let spec = KernelSpec {
                argv: vec![
                    String::from(exe_path.to_string_lossy()),
                    String::from("--connection_file"),
                    String::from("{connection_file}"),
                ],
                language: String::from("Echo"),
                display_name: String::from("Amalthea Echo"),
                env: serde_json::Map::new(),
            };
            if let Err(err) = spec.install(String::from("amalthea")) {
                eprintln!("Failed to install Jupyter kernelspec. {}", err);
            } else {
                println!("Successfully installed Jupyter kernelspec.")
            }
        },
        Err(err) => {
            eprintln!("Failed to determine path to Amalthea. {}", err);
        },
    }
}

fn main() {
    // Initialize logging system; the env_logger lets you configure loggign with
    // the RUST_LOG env var
    env_logger::init();

    // Get an iterator over all the command-line arguments
    let mut argv = std::env::args();

    // Skip the first "argument" as it's the path/name to this executable
    argv.next();

    // Process remaining arguments
    match argv.next() {
        Some(arg) => match arg.as_str() {
            "--connection_file" => {
                if let Some(file) = argv.next() {
                    let (connection_file, registration_file) =
                        kernel::read_connection(file.as_str());
                    start_kernel(connection_file, registration_file);
                } else {
                    eprintln!(
                        "A connection file must be specified with the --connection_file argument."
                    );
                }
            },
            "--version" => {
                println!("Amalthea {}", env!("CARGO_PKG_VERSION"));
            },
            "--install" => {
                install_kernel_spec();
            },
            other => {
                eprintln!("Argument '{}' unknown", other);
            },
        },
        None => {
            println!("Usage: amalthea --connection_file /path/to/file");
        },
    }
}
