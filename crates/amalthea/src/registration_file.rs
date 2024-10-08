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

use crate::wire::jupyter_message::Status;

/// The contents of the Registration File as implied in JEP 66.
#[derive(Deserialize, Debug)]
pub(crate) struct RegistrationFile {
    /// The transport type to use for ZeroMQ; generally "tcp"
    pub(crate) transport: String,

    /// The signature scheme to use for messages; generally "hmac-sha256"
    pub(crate) signature_scheme: String,

    /// The IP address to bind to
    pub(crate) ip: String,

    /// The HMAC-256 signing key, or an empty string for an unauthenticated
    /// connection
    pub(crate) key: String,

    /// ZeroMQ port: Registration messages (handshake)
    pub(crate) registration_port: u16,
}

/// The handshake request
#[derive(Deserialize, Debug)]
pub(crate) struct HandshakeRequest {
    /// ZeroMQ port: Control channel (kernel interrupts)
    pub(crate) control_port: u16,

    /// ZeroMQ port: Shell channel (execution, completion)
    pub(crate) shell_port: u16,

    /// ZeroMQ port: Standard input channel (prompts)
    pub(crate) stdin_port: u16,

    /// ZeroMQ port: IOPub channel (broadcasts input/output)
    pub(crate) iopub_port: u16,

    /// ZeroMQ port: Heartbeat messages (echo)
    pub(crate) hb_port: u16,
}

/// The handshake reply
#[derive(Deserialize, Debug)]
pub(crate) struct HandshakeReply {
    pub(crate) status: Status,
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
