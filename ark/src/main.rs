/*
 * main.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::shell::Shell;
use amalthea::connection_file::ConnectionFile;
use amalthea::kernel::Kernel;
use amalthea::kernel_spec::KernelSpec;
use amalthea::socket::iopub::IOPubMessage;
use log::{debug, error, info};
use std::env;
use std::io::stdin;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::thread;

mod lsp;
mod r_interface;
mod r_kernel;
mod shell;

fn start_kernel(connection_file: ConnectionFile) {
    // This channel delivers execution status and other iopub messages from
    // other threads to the iopub thread

    let (iopub_sender, iopub_receiver) = channel::<IOPubMessage>();

    let shell_sender = iopub_sender.clone();
    let shell = Arc::new(Mutex::new(Shell::new(shell_sender)));

    // Start the LSP backend
    thread::spawn(move || lsp::backend::start_lsp(9277));

    let kernel = Kernel::new(connection_file);
    match kernel {
        Ok(k) => match k.connect(shell, iopub_sender, iopub_receiver) {
            Ok(()) => {
                let mut s = String::new();
                println!("R Kernel exiting.");
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

fn install_kernel_spec() {
    match env::current_exe() {
        Ok(exe_path) => {
            let spec = KernelSpec {
                argv: vec![
                    String::from(exe_path.to_string_lossy()),
                    String::from("--connection_file"),
                    String::from("{connection_file}"),
                ],
                language: String::from("R"),
                display_name: String::from("Amalthea R Kernel (ARK)"),
            };
            if let Err(err) = spec.install(String::from("ark")) {
                eprintln!("Failed to install Ark's Jupyter kernelspec. {}", err);
            } else {
                println!("Successfully installed Ark Jupyter kernelspec.")
            }
        }
        Err(err) => {
            eprintln!("Failed to determine path to Ark. {}", err);
        }
    }
}

fn parse_file(connection_file: &String) {
    match ConnectionFile::from_file(connection_file) {
        Ok(connection) => {
            info!(
                "Loaded connection information from front end in {}",
                connection_file
            );
            debug!("Connection data: {:?}", connection);
            start_kernel(connection);
        }
        Err(error) => {
            error!(
                "Couldn't read connection file {}: {:?}",
                connection_file, error
            );
        }
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

    // Process remaining arguments. TODO: Need an argument that can passthrough args to R
    match argv.next() {
        Some(arg) => {
            match arg.as_str() {
                "--connection_file" => {
                    if let Some(file) = argv.next() {
                        parse_file(&file);
                    } else {
                        eprintln!("A connection file must be specified with the --connection_file argument.");
                    }
                }
                "--version" => {
                    println!("Ark {}", env!("CARGO_PKG_VERSION"));
                }
                "--install" => {
                    install_kernel_spec();
                }
                other => {
                    eprintln!("Argument '{}' unknown", other);
                }
            }
        }
        None => {
            println!("Ark {}, the Amalthea R Kernel.", env!("CARGO_PKG_VERSION"));
            println!("Usage: ark --connection_file /path/to/file");
        }
    }
}
