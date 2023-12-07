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

use amalthea::connection_file::ConnectionFile;
use amalthea::kernel::Kernel;
use amalthea::kernel::StreamBehavior;
use amalthea::kernel_spec::KernelSpec;
use amalthea::socket::stdin::StdInRequest;
use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use log::debug;
use log::error;
use log::info;

use crate::control::Control;
use crate::shell::Shell;

fn start_kernel(connection_file: ConnectionFile) {
    let mut kernel = match Kernel::new("echo", connection_file) {
        Ok(k) => k,
        Err(err) => {
            log::error!("Failed to create kernel: {err:?}");
            return;
        },
    };

    // Communication channel with StdIn
    let (stdin_request_tx, stdin_request_rx) = bounded::<StdInRequest>(1);
    let (input_reply_tx, input_reply_rx) = unbounded();

    let shell_tx = kernel.create_iopub_tx();
    let shell = Arc::new(Mutex::new(Shell::new(
        shell_tx,
        stdin_request_tx,
        input_reply_rx,
    )));
    let control = Arc::new(Mutex::new(Control {}));

    if let Err(err) = kernel.connect(
        shell,
        control,
        None,
        None,
        StreamBehavior::None,
        stdin_request_rx,
        input_reply_tx,
    ) {
        panic!("Couldn't connect to front end: {err:?}");
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

fn parse_file(connection_file: &String) {
    match ConnectionFile::from_file(connection_file) {
        Ok(connection) => {
            info!(
                "Loaded connection information from front-end in {}",
                connection_file
            );
            debug!("Connection data: {:?}", connection);
            start_kernel(connection);
        },
        Err(error) => {
            error!(
                "Couldn't read connection file {}: {:?}",
                connection_file, error
            );
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
        Some(arg) => {
            match arg.as_str() {
                "--connection_file" => {
                    if let Some(file) = argv.next() {
                        parse_file(&file);
                    } else {
                        eprintln!("A connection file must be specified with the --connection_file argument.");
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
            }
        },
        None => {
            println!("Usage: amalthea --connection_file /path/to/file");
        },
    }
}
