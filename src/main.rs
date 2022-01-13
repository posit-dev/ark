/*
 * main.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

mod connection_file;
mod heartbeat;
mod kernel;

use crate::connection_file::ConnectionFile;
use crate::kernel::Kernel;

fn start_kernel(connection_file: ConnectionFile) {
    let kernel = Kernel::create(connection_file);
    match kernel {
        Ok(k) => {
            match k.connect() {
                Ok(()) => {
                    // TODO: Just let it run
                }
                Err(err) => {
                    panic!("Couldn't connect to front end: {:?}", err);
                }
            }
        }
        Err(err) => {
            panic!("Couldn't create kernel: {:?}", err);
        }
    }
}

fn parse_file(connection_file: &String) {
    match ConnectionFile::from_file(connection_file) {
        Ok(connection) => {
            println!("Connection data: {:?}", connection);
            start_kernel(connection);
        }
        Err(error) => {
            panic!("Couldn't read {}: {:?}", connection_file, error);
        }
    }
}

fn main() {
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
                        println!("Loading connection file {}", file);
                        parse_file(&file);
                    } else {
                        eprintln!("A connection file must be specified with the --connection_file argument.");
                    }
                }
                "--version" => {
                    println!("Amalthea {}", env!("CARGO_PKG_VERSION"));
                }
                other => {
                    eprintln!("Argument '{}' unknown", other);
                }
            }
        }
        None => {
            println!("Usage: amalthea --connection_file /path/to/file");
        }
    }
}
