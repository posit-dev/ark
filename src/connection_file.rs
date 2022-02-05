/*
 * connection_file.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use serde::Deserialize;
use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

#[derive(Deserialize, Debug)]
pub struct ConnectionFile {
    // ZeroMQ ports
    pub control_port: u16,
    pub shell_port: u16,
    pub stdin_port: u16,
    pub iopub_port: u16,
    pub hb_port: u16,

    // TODO: enum? "tcp"
    pub transport: String,
    // TODO: enum? "hmac-sha256"
    pub signature_scheme: String,

    pub ip: String,
    pub key: String,
}

impl ConnectionFile {
    /// Create a ConnectionFile by parsing the contents of a connection file.
    pub fn from_file<P: AsRef<Path>>(connection_file: P) -> Result<ConnectionFile, Box<dyn Error>> {
        let file = File::open(connection_file)?;
        let reader = BufReader::new(file);
        let control = serde_json::from_reader(reader)?;

        Ok(control)
    }

    /// Given a port, return a URI-like string that can be used to connect to
    /// the port, given the other parameters in the connection file.
    ///
    /// Example: `32` => `"tcp://127.0.0.1:32"`
    pub fn endpoint(&self, port: u16) -> String {
        format!("{}://{}:{}", self.transport, self.ip, port)
    }
}
