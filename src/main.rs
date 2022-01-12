/*
 * main.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use serde::Deserialize;

#[derive(Deserialize)]
struct ControlFile {
    // ZeroMQ ports
    control_port: u16,
    shell_port: u16,
    stdin_port: u16,
    iopub_port: u16,
    hb_port: u16,

    // TODO: enum? "tcp"
    transport: String,
    // TODO: enum? "hmac-sha256"
    signature_scheme: String,

    ip: String,
    key: String
}

fn main() {
    println!("Amalthea: An R kernel for Myriac and Jupyter.");

    // Get an iterator over all the command-line arguments
    let mut argv = std::env::args();

    // Skip the first "argument" as it's the path/name to this executable
    argv.next();

    // Process remaining arguments
    match argv.next() {
        Some(arg) => {
            match arg.as_str() {
                "--control_file" => {
                    // TODO: handle missing control file
                    println!("Loading control file {}", argv.next().unwrap())
                },
                other => {
                    eprintln!("Argument '{}' unknown", other);
                }
            }
        }
        None => {
            println!("Usage: amalthea --control_file /path/to/file");
        }
    } 
}

