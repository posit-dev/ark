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
    key: String,
}

impl ConnectionFile {
    /// Create a ConnectionFile by parsing the contents of a connection file.
    pub fn from_file<P: AsRef<Path>>(connection_file: P) -> Result<ConnectionFile, Box<dyn Error>> {
        let file = File::open(connection_file)?;
        let reader = BufReader::new(file);
        let control = serde_json::from_reader(reader)?;

        Ok(control)
    }
}
