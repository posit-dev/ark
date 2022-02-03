/*
 * kernel.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::connection_file::ConnectionFile;
use crate::error::Error;
use crate::session::Session;
use crate::socket::heartbeat::Heartbeat;
use crate::socket::iopub::IOPub;
use crate::socket::shell::Shell;
use crate::socket::socket::connect;

pub struct Kernel {
    /// The connection metadata
    connection: ConnectionFile,

    /// The session connection information
    session: Session,

    heartbeat: Heartbeat,
}

impl Kernel {
    /// Create a new Kernel, given a connection file from a front end.
    pub fn create(file: ConnectionFile) -> Result<Kernel, Error> {
        let key = file.key.clone();
        Ok(Self {
            connection: file,
            session: Session::create(key)?,
            heartbeat: Heartbeat {},
        })
    }

    /// Connect the Kernel to the front end.
    pub fn connect(&self) -> Result<(), Error> {
        let ctx = zmq::Context::new();
        self.heartbeat
            .connect(&ctx, self.endpoint(self.connection.hb_port))?;
        connect::<Shell>(
            &ctx,
            self.endpoint(self.connection.shell_port),
            self.session.clone(),
        )?;
        connect::<IOPub>(
            &ctx,
            self.endpoint(self.connection.iopub_port),
            self.session.clone(),
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
