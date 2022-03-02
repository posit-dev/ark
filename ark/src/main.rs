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
use libc::{c_char, c_int, c_void};
use log::{debug, error, info};
use std::env;
use std::ffi::CString;
use std::io::stdin;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

mod shell;

#[link(name = "R", kind = "dylib")]
extern "C" {
    // TODO: this is actually a vector of cstrings
    fn Rf_initialize_R(ac: c_int, av: *const c_char) -> i32;

    /// Global indicating whether R is running as the main program (affects
    /// R_CStackStart)
    static mut R_running_as_main_program: c_int;

    /// Flag indicating whether this is an interactive session. R typically sets
    /// this when attached to a tty.
    static mut R_Interactive: c_int;

    /// Pointer to file receiving console input
    static mut R_Consolefile: *const c_void;

    /// Pointer to file receiving output
    static mut R_Outputfile: *const c_void;

    // TODO: type of buffer isn't necessary c_char
    static mut ptr_R_ReadConsole:
        unsafe extern "C" fn(*const c_char, *const c_char, i32, i32) -> i32;
}

#[no_mangle]
pub extern "C" fn r_read_console(
    _prompt: *const c_char,
    _buf: *const c_char,
    _buflen: i32,
    _hist: i32,
) -> i32 {
    0
}

fn start_kernel(connection_file: ConnectionFile) {
    let args = CString::new("").unwrap();

    // TODO: Discover R locations and populate R_HOME, a prerequisite to
    // initializing R.
    //
    // Maybe add a command line option to specify the path to R_HOME directly?
    unsafe {
        R_running_as_main_program = 1;
        R_Interactive = 1;
        R_Consolefile = std::ptr::null();
        R_Outputfile = std::ptr::null();
        ptr_R_ReadConsole = r_read_console;
        Rf_initialize_R(0, args.as_ptr());
    }

    // This channel delivers execution status and other iopub messages from
    // other threads to the iopub thread

    let (iopub_sender, iopub_receiver) = channel::<IOPubMessage>();

    let shell_sender = iopub_sender.clone();
    let shell = Arc::new(Mutex::new(Shell::new(shell_sender)));

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
                language: String::from("Echo"),
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
