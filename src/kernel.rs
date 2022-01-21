/*
 * kernel.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::connection_file::ConnectionFile;
use crate::socket::heartbeat::Heartbeat;
use crate::socket::shell::Shell;

pub struct Kernel {
    /// The connection metadata
    connection: ConnectionFile,
    heartbeat: Heartbeat,
    shell: Shell,
}

impl Kernel {
    /// Create a new Kernel, given a connection file from a front end.
    pub fn create(file: ConnectionFile) -> Result<Kernel, zmq::Error> {
        Ok(Self {
            connection: file,
            heartbeat: Heartbeat {},
            shell: Shell {},
        })
    }

    /// Connect the Kernel to the front end.
    pub fn connect(&self) -> Result<(), zmq::Error> {
        let ctx = zmq::Context::new();
        self.heartbeat
            .connect(&ctx, self.endpoint(self.connection.hb_port))?;
        self.shell
            .connect(&ctx, self.endpoint(self.connection.shell_port))?;
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
