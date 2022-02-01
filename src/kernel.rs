/*
 * kernel.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::connection_file::ConnectionFile;
use crate::error::Error;
use crate::socket::heartbeat::Heartbeat;
use crate::socket::shell::Shell;
use hmac::{Hmac, Mac};
use sha2::Sha256;

pub struct Kernel {
    /// The connection metadata
    connection: ConnectionFile,

    /// The HMAC signing key, if any
    hmac: Option<Hmac<Sha256>>,

    heartbeat: Heartbeat,
    shell: Shell,
}

impl Kernel {
    /// Create a new Kernel, given a connection file from a front end.
    pub fn create(file: ConnectionFile) -> Result<Kernel, Error> {
        let key = match file.key.len() {
            0 => None,
            _ => {
                let result = match Hmac::<Sha256>::new_from_slice(file.key.as_bytes()) {
                    Ok(hmac) => hmac,
                    Err(err) => return Err(Error::HmacKeyInvalid(file.key, err)),
                };
                Some(result)
            }
        };
        Ok(Self {
            connection: file,
            heartbeat: Heartbeat {},
            hmac: key,
            shell: Shell {},
        })
    }

    /// Connect the Kernel to the front end.
    pub fn connect(&self) -> Result<(), zmq::Error> {
        let ctx = zmq::Context::new();
        self.heartbeat
            .connect(&ctx, self.endpoint(self.connection.hb_port))?;
        self.shell.connect(
            &ctx,
            self.hmac.clone(),
            self.endpoint(self.connection.shell_port),
        )?;
        Ok(())
    }

    /// Given a port, return a URI-like string that can be used to connect to
    /// the port, given the other parameters in the connection file.
    ///
    /// Example: `32` => `"tcp://127.0.0.1:32"`
    fn endpoint(&self, port: u16) -> String {
        format!(
            "{}://{}:{}",
            self.connection.transport, self.connection.ip, port
        )
    }
}
