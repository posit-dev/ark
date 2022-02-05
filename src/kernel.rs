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
use crate::wire::status::ExecutionState;
use std::rc::Rc;
use std::sync::mpsc::channel;

pub struct Kernel {
    /// The connection metadata
    connection: ConnectionFile,

    /// The session connection information
    session: Session,

    /// The heartbeat socket
    heartbeat: Heartbeat,
}

impl Kernel {
    /// Create a new Kernel, given a connection file from a front end.
    pub fn create(file: ConnectionFile) -> Result<Kernel, Error> {
        let key = file.key.clone();

        let ctx = zmq::Context::new();

        let heartbeat = Heartbeat {};
        heartbeat.connect(&ctx, file.endpoint(file.hb_port))?;

        let session = Session::create(key)?;

        // Create the sockets
        let shell_socket = Rc::new(connect::<Shell>(
            &ctx,
            file.endpoint(file.shell_port),
            session.clone(),
        )?);
        let iopub_socket = Rc::new(connect::<IOPub>(
            &ctx,
            file.endpoint(file.iopub_port),
            session.clone(),
        )?);

        // This channel delivers execution status from other threads to the iopub thread
        let (status_sender, status_receiver) = channel::<ExecutionState>();

        let iopub = IOPub::new(iopub_socket, status_receiver);
        let shell = Shell::new(shell_socket, status_sender.clone());

        Ok(Self {
            connection: file,
            session: Session::create(key)?,
            heartbeat: Heartbeat {},
        })
    }

    pub fn connect() -> Result<(), Error> {}
}
