/*
 * kernel.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::connection_file::ConnectionFile;
use crate::error::Error;
use crate::language::executor::Executor;
use crate::session::Session;
use crate::socket::heartbeat::Heartbeat;
use crate::socket::iopub::IOPub;
use crate::socket::shell::Shell;
use crate::socket::signed_socket::SignedSocket;
use crate::wire::jupyter_message::Message;
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

        // This channel delivers execution status and other iopub messages from
        // other threads to the iopub thread
        let (iopub_sender, iopub_receiver) = channel::<Message>();

        // These pair of channels are used for execution requests, which
        // coordinate execution between the shell thread and the language
        // execution thread
        let (exec_req_send, exec_req_recv) = channel::<Message>();
        let (exec_rep_send, exec_rep_recv) = channel::<Message>();

        let shell_socket = SignedSocket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Shell"),
            zmq::ROUTER,
            self.connection.endpoint(self.connection.shell_port),
        )?;
        thread::spawn(move || {
            Self::shell_thread(shell_socket, iopub_sender, exec_req_send, exec_rep_recv)
        });

        let iopub_socket = SignedSocket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("IOPub"),
            zmq::PUB,
            self.connection.endpoint(self.connection.iopub_port),
        )?;
        let exec_socket = iopub_socket.clone();
        thread::spawn(move || Self::execution_thread(exec_socket, exec_rep_send, exec_req_recv));
        thread::spawn(move || Self::iopub_thread(iopub_socket, iopub_receiver));

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
        iopub_sender: Sender<Message>,
        request_sender: Sender<Message>,
        reply_receiver: Receiver<Message>,
    ) -> Result<(), Error> {
        let mut shell = Shell::new(socket, iopub_sender.clone(), request_sender, reply_receiver);
        shell.listen();
        Ok(())
    }

    fn iopub_thread(socket: SignedSocket, receiver: Receiver<Message>) -> Result<(), Error> {
        let iopub = IOPub::new(socket, receiver);
        iopub.listen();
        Ok(())
    }

    fn heartbeat_thread(socket: SignedSocket) -> Result<(), Error> {
        let mut heartbeat = Heartbeat::new(socket);
        heartbeat.listen();
        Ok(())
    }

    fn execution_thread(
        iopub: SignedSocket,
        sender: Sender<Message>,
        receiver: Receiver<Message>,
    ) -> Result<(), Error> {
        let mut executor = Executor::new(iopub, sender, receiver);
        executor.listen();
        Ok(())
    }
}
