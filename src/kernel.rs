/*
 * kernel.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::connection_file::ConnectionFile;

pub struct Kernel {
    /// The connection metadata
    connection: ConnectionFile,

    heartbeat: zmq::Socket,

    /// The ZeroMQ connection context
    context: zmq::Context,
}

impl Kernel {
    /// Create a new Kernel, given a connection file from a front end.
    pub fn create(file: ConnectionFile) -> Result<Kernel, zmq::Error> {
        let ctx = zmq::Context::new();
        let heartbeat = ctx.socket(zmq::REQ)?;
        Ok(Self {
            connection: file,
            heartbeat: heartbeat,
            context: ctx,
        })
    }

    /// Connect the Kernel to the front end.
    pub fn connect(&self) -> Result<(), zmq::Error> {
        self.heartbeat.connect(&String::from(format!(
            "{}:{}",
            self.connection.ip, self.connection.hb_port
        )))?;
        Ok(())
    }
}
