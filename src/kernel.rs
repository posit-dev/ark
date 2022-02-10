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
use crate::socket::socket_channel::SocketChannel;
use crate::wire::status::ExecutionState;
use std::rc::Rc;
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

        let shell_socket = Arc::new(connect::<Shell>(&ctx, self.connection.endpoint(self.connection.shell_port), self.session.clone())?);
        let shell_channel = SocketChannel::new();
        let shell_endpoint = ;
        let session = self.session.clone();
        let shell_ctx = ctx.clone();
        thread::spawn(move || {
            Self::shell_thread(shell_ctx, shell_endpoint, status_sender, session)
        });

        let iopub_endpoint = self.connection.endpoint(self.connection.iopub_port);
        let session = self.session.clone();
        let iopub_ctx = ctx.clone();
        thread::spawn(move || {
            Self::iopub_thread(iopub_ctx, iopub_endpoint, status_receiver, session)
        });

        let heartbeat_endpoint = self.connection.endpoint(self.connection.hb_port);
        let session = self.session.clone();
        let hb_ctx = ctx.clone();
        thread::spawn(move || Self::heartbeat_thread(hb_ctx, heartbeat_endpoint, session));
        Ok(())
    }

    fn shell_thread(
        ctx: zmq::Context,
        endpoint: String,
        status_sender: Sender<ExecutionState>,
        session: Session,
    ) -> Result<(), Error> {
        let shell_socket = Rc::new(connect::<Shell>(&ctx, endpoint, session.clone())?);
        let mut shell = Shell::new(shell_socket, status_sender.clone());
        shell.listen();
        Ok(())
    }

    fn iopub_thread(
        ctx: zmq::Context,
        endpoint: String,
        status_receiver: Receiver<ExecutionState>,
        session: Session,
    ) -> Result<(), Error> {
        let iopub_socket = Rc::new(connect::<IOPub>(&ctx, endpoint, session.clone())?);
        let mut iopub = IOPub::new(iopub_socket, status_receiver);
        iopub.listen();
        Ok(())
    }

    fn heartbeat_thread(
        ctx: zmq::Context,
        endpoint: String,
        session: Session,
    ) -> Result<(), Error> {
        let heartbeat_socket = Rc::new(connect::<Heartbeat>(&ctx, endpoint, session.clone())?);
        let mut heartbeat = Heartbeat::new(heartbeat_socket);
        heartbeat.listen();
        Ok(())
    }
}
