/*
 * registration_file.rs
 *
 * Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *
 */

use std::error::Error;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use serde::Deserialize;

use crate::connection_file::ConnectionFile;

/// The contents of the Registration File as implied in JEP 66.
#[derive(Deserialize, Debug)]
pub struct RegistrationFile {
    /// The transport type to use for ZeroMQ; generally "tcp"
    pub transport: String,

    /// The signature scheme to use for messages; generally "hmac-sha256"
    pub signature_scheme: String,

    /// The IP address to bind to
    pub ip: String,

    /// The HMAC-256 signing key, or an empty string for an unauthenticated
    /// connection
    pub key: String,

    /// ZeroMQ port: Registration messages (handshake)
    pub registration_port: u16,
}

impl RegistrationFile {
    /// Create a RegistrationFile by parsing the contents of a registration file.
    pub fn from_file<P: AsRef<Path>>(
        registration_file: P,
    ) -> Result<RegistrationFile, Box<dyn Error>> {
        let file = File::open(registration_file)?;
        let reader = BufReader::new(file);
        let control = serde_json::from_reader(reader)?;

        Ok(control)
    }

    pub fn as_connection_file(&self) -> ConnectionFile {
        // `0` stands for zeromq / OS picking an available port
        let control_port = 0;
        let shell_port = 0;
        let stdin_port = 0;
        let iopub_port = 0;
        let hb_port = 0;

        // Build a `ConnectionFile`
        let connection = ConnectionFile {
            control_port,
            shell_port,
            stdin_port,
            iopub_port,
            hb_port,
            transport: self.transport.clone(),
            signature_scheme: self.signature_scheme.clone(),
            ip: self.ip.clone(),
            key: self.key.clone(),
        };

        connection
    }

    /// Given a port, return a URI-like string that can be used to connect to
    /// the port, given the other parameters in the connection file.
    ///
    /// Example: `32` => `"tcp://127.0.0.1:32"`
    pub fn endpoint(&self) -> String {
        format!(
            "{}://{}:{}",
            self.transport, self.ip, self.registration_port
        )
    }
}
