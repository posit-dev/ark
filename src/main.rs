/*
 * main.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

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

