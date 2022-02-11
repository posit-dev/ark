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
use crate::socket::signed_socket::SignedSocket;
use crate::wire::status::ExecutionState;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

pub struct Kernel {
    /// The connection metadata
    connection: ConnectionFile,
    session: Session,
}

impl Kernel {
    /// Create a new Kernel, given a connection file from a front end.
    pub fn new(file: ConnectionFile) -> Result<Kernel, Error> {
        let key = file.key.clone();

        Ok(Self {
            connection: file,
            session: Session::create(key)?,
        })
    }

    pub fn connect(&self) -> Result<(), Error> {
        let ctx = zmq::Context::new();

        // This channel delivers execution status from other threads to the iopub thread
        let (status_sender, status_receiver) = channel::<ExecutionState>();

        let shell_socket = SignedSocket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Shell"),
            zmq::ROUTER,
            self.connection.endpoint(self.connection.shell_port),
        )?;
        thread::spawn(move || Self::shell_thread(shell_socket, status_sender));

        let iopub_socket = SignedSocket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("IOPub"),
            zmq::PUB,
            self.connection.endpoint(self.connection.iopub_port),
        )?;
        thread::spawn(move || Self::iopub_thread(iopub_socket, status_receiver));

        let heartbeat_socket = SignedSocket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Heartbeat"),
            zmq::REQ,
            self.connection.endpoint(self.connection.hb_port),
        )?;
        thread::spawn(move || Self::heartbeat_thread(heartbeat_socket));
        Ok(())
    }

    fn shell_thread(
        socket: SignedSocket,
        status_sender: Sender<ExecutionState>,
    ) -> Result<(), Error> {
        let mut shell = Shell::new(socket, status_sender.clone());
        shell.listen();
        Ok(())
    }

    fn iopub_thread(
        socket: SignedSocket,
        status_receiver: Receiver<ExecutionState>,
    ) -> Result<(), Error> {
        let mut iopub = IOPub::new(socket, status_receiver);
        iopub.listen();
        Ok(())
    }

    fn heartbeat_thread(socket: SignedSocket) -> Result<(), Error> {
        let mut heartbeat = Heartbeat::new(socket);
        heartbeat.listen();
        Ok(())
    }
}
