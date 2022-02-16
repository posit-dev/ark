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
use crate::socket::control::Control;
use crate::socket::heartbeat::Heartbeat;
use crate::socket::iopub::IOPub;
use crate::socket::shell::Shell;
use crate::socket::socket::Socket;
use crate::wire::jupyter_message::Message;
use log::trace;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

/// A Kernel represents a unique Jupyter kernel session and is the host for all
/// execution and messaging threads.
pub struct Kernel {
    /// The connection metadata.
    connection: ConnectionFile,

    /// The unique session information for this kernel session.
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

    /// Connects the Kernel to the front end.
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

        // Create the Shell ROUTER/DEALER socket and start a thread to listen
        // for client messages.
        let shell_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Shell"),
            zmq::ROUTER,
            self.connection.endpoint(self.connection.shell_port),
        )?;
        let shell_sender = iopub_sender.clone();
        thread::spawn(move || {
            Self::shell_thread(shell_socket, shell_sender, exec_req_send, exec_rep_recv)
        });

        // Create the IOPub PUB/SUB socket and start a thread to broadcast to
        // the client. IOPub only broadcasts messages, so it listens to other
        // threads on a Receiver<Message> instead of to the client.
        let iopub_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("IOPub"),
            zmq::PUB,
            self.connection.endpoint(self.connection.iopub_port),
        )?;
        thread::spawn(move || Self::iopub_thread(iopub_socket, iopub_receiver));

        // Create the heartbeat socket and start a thread to listen for
        // heartbeat messages.
        let heartbeat_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Heartbeat"),
            zmq::REP,
            self.connection.endpoint(self.connection.hb_port),
        )?;
        thread::spawn(move || Self::heartbeat_thread(heartbeat_socket));

        // Create the execution thread. This is the thread on which actual
        // language execution happens.
        let session = self.session.clone();
        trace!("Spawning execution thread...");
        thread::spawn(move || {
            trace!("...thread spawned");
            Self::execution_thread(session, iopub_sender, exec_rep_send, exec_req_recv)
        });

        // Create the Control ROUTER/DEALER socket
        let control_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Control"),
            zmq::ROUTER,
            self.connection.endpoint(self.connection.control_port),
        )?;
        thread::spawn(move || Self::control_thread(control_socket));
        Ok(())
    }

    /// Starts the control thread
    fn control_thread(socket: Socket) {
        let control = Control::new(socket);
        control.listen();
    }

    /// Starts the shell thread.
    fn shell_thread(
        socket: Socket,
        iopub_sender: Sender<Message>,
        request_sender: Sender<Message>,
        reply_receiver: Receiver<Message>,
    ) -> Result<(), Error> {
        let mut shell = Shell::new(socket, iopub_sender.clone(), request_sender, reply_receiver);
        shell.listen();
        Ok(())
    }

    /// Starts the IOPub thread.
    fn iopub_thread(socket: Socket, receiver: Receiver<Message>) -> Result<(), Error> {
        let iopub = IOPub::new(socket, receiver);
        iopub.listen();
        Ok(())
    }

    /// Starts the heartbeat thread.
    fn heartbeat_thread(socket: Socket) -> Result<(), Error> {
        let mut heartbeat = Heartbeat::new(socket);
        heartbeat.listen();
        Ok(())
    }

    /// Starts the language execution thread.
    fn execution_thread(
        session: Session,
        iopub_sender: Sender<Message>,
        sender: Sender<Message>,
        receiver: Receiver<Message>,
    ) -> Result<(), Error> {
        let mut executor = Executor::new(session, iopub_sender, sender, receiver);
        executor.listen();
        Ok(())
    }
}
