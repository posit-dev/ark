/*
 * mod.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::connection_file::ConnectionFile;
use zmq::Socket;

pub struct Frontend {}

impl Frontend {
    pub fn new() -> Self {
        Self {}
    }

    pub fn get_connection_file(&self) -> ConnectionFile {
        ConnectionFile {
            control_port: 0,
            shell_port: 0,
            stdin_port: 0,
            iopub_port: 0,
            hb_port: 0,
            transport: String::from("tcp"),
            signature_scheme: String::from("hmac-sha256"),
            ip: String::from("127.0.0.1"),
            key: String::from(""), // TODO: generate this!
        }
    }
}
