/*
 * kernel.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::sync::Arc;
use std::sync::Mutex;

use crate::comm::comm_manager::CommManager;
use crate::comm::event::CommChanged;
use crate::comm::event::CommEvent;
use crate::connection_file::ConnectionFile;
use crate::error::Error;
use crate::language::control_handler::ControlHandler;
use crate::language::lsp_handler::LspHandler;
use crate::language::shell_handler::ShellHandler;
use crate::session::Session;
use crate::socket::control::Control;
use crate::socket::heartbeat::Heartbeat;
use crate::socket::iopub::IOPub;
use crate::socket::iopub::IOPubMessage;
use crate::socket::shell::Shell;
use crate::socket::socket::Socket;
use crate::socket::stdin::Stdin;
use crate::stream_capture::StreamCapture;

use crossbeam::channel::bounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use log::info;
use stdext::spawn;

/// A Kernel represents a unique Jupyter kernel session and is the host for all
/// execution and messaging threads.
pub struct Kernel {
    /// The name of the kernel.
    name: String,

    /// The connection metadata.
    connection: ConnectionFile,

    /// The unique session information for this kernel session.
    session: Session,

    /// Sends messages to the IOPub socket. This field is used throughout the
    /// kernel codebase to send events to the front end; use `create_iopub_tx`
    /// to access it.
    iopub_tx: Sender<IOPubMessage>,

    /// Receives message sent to the IOPub socket
    iopub_rx: Option<Receiver<IOPubMessage>>,

    /// Sends notifications about comm changes and events to the comm manager.
    /// Use `create_comm_manager_tx` to access it.
    comm_manager_tx: Sender<CommEvent>,

    /// Receives notifications about comm changes and events
    comm_manager_rx: Receiver<CommEvent>,
}

/// Possible behaviors for the stream capture thread. When set to `Capture`,
/// the stream capture thread will capture all output to stdout and stderr.
/// When set to `None`, no stream output is captured.
#[derive(PartialEq)]
pub enum StreamBehavior {
    Capture,
    None,
}

impl Kernel {
    /// Create a new Kernel, given a connection file from a front end.
    pub fn new(name: &str, file: ConnectionFile) -> Result<Kernel, Error> {
        let key = file.key.clone();

        let (iopub_tx, iopub_rx) = bounded::<IOPubMessage>(10);

        // Create the pair of channels that will be used to relay messages from
        // the open comms
        let (comm_manager_tx, comm_manager_rx) = bounded::<CommEvent>(10);

        Ok(Self {
            name: name.to_string(),
            connection: file,
            session: Session::create(key)?,
            iopub_tx: iopub_tx,
            iopub_rx: Some(iopub_rx),
            comm_manager_tx,
            comm_manager_rx,
        })
    }

    /// Connects the Kernel to the front end
    pub fn connect(
        &mut self,
        shell_handler: Arc<Mutex<dyn ShellHandler>>,
        control_handler: Arc<Mutex<dyn ControlHandler>>,
        lsp_handler: Option<Arc<Mutex<dyn LspHandler>>>,
        stream_behavior: StreamBehavior,
    ) -> Result<(), Error> {
        let ctx = zmq::Context::new();

        // Create the comm manager thread
        let iopub_tx = self.create_iopub_tx();
        let comm_manager_rx = self.comm_manager_rx.clone();
        let comm_changed_rx = CommManager::start(iopub_tx, comm_manager_rx);

        // Create the Shell ROUTER/DEALER socket and start a thread to listen
        // for client messages.
        let shell_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Shell"),
            zmq::ROUTER,
            None,
            self.connection.endpoint(self.connection.shell_port),
        )?;

        let shell_clone = shell_handler.clone();
        let iopub_tx_clone = self.create_iopub_tx();
        let comm_manager_tx_clone = self.comm_manager_tx.clone();
        let lsp_handler_clone = lsp_handler.clone();
        spawn!(format!("{}-shell", self.name), move || {
            Self::shell_thread(
                shell_socket,
                iopub_tx_clone,
                comm_manager_tx_clone,
                comm_changed_rx,
                shell_clone,
                lsp_handler_clone,
            )
        });

        // Create the IOPub PUB/SUB socket and start a thread to broadcast to
        // the client. IOPub only broadcasts messages, so it listens to other
        // threads on a Receiver<Message> instead of to the client.
        let iopub_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("IOPub"),
            zmq::PUB,
            None,
            self.connection.endpoint(self.connection.iopub_port),
        )?;
        let iopub_rx = self.iopub_rx.take().unwrap();
        spawn!(format!("{}-iopub", self.name), move || {
            Self::iopub_thread(iopub_socket, iopub_rx)
        });

        // Create the heartbeat socket and start a thread to listen for
        // heartbeat messages.
        let heartbeat_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Heartbeat"),
            zmq::REP,
            None,
            self.connection.endpoint(self.connection.hb_port),
        )?;
        spawn!(format!("{}-heartbeat", self.name), move || {
            Self::heartbeat_thread(heartbeat_socket)
        });

        // Create the stdin socket and start a thread to listen for stdin
        // messages. These are used by the kernel to request input from the
        // user, and so flow in the opposite direction to the other sockets.
        let stdin_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Stdin"),
            zmq::ROUTER,
            None,
            self.connection.endpoint(self.connection.stdin_port),
        )?;
        let shell_clone = shell_handler.clone();
        spawn!(format!("{}-stdin", self.name), move || {
            Self::stdin_thread(stdin_socket, shell_clone)
        });

        // Create the thread that handles stdout and stderr, if requested
        if stream_behavior == StreamBehavior::Capture {
            let iopub_tx = self.create_iopub_tx();
            spawn!(format!("{}-output-capture", self.name), move || {
                Self::output_capture_thread(iopub_tx)
            });
        }

        // Create the Control ROUTER/DEALER socket
        let control_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Control"),
            zmq::ROUTER,
            None,
            self.connection.endpoint(self.connection.control_port),
        )?;

        // TODO: thread/join thread? Exiting this thread will cause the whole
        // kernel to exit.
        Self::control_thread(control_socket, control_handler);
        info!("Control thread exited, exiting kernel");
        Ok(())
    }

    /// Returns a copy of the IOPub sending channel.
    pub fn create_iopub_tx(&self) -> Sender<IOPubMessage> {
        self.iopub_tx.clone()
    }

    /// Returns a copy of the comm manager sending channel.
    pub fn create_comm_manager_tx(&self) -> Sender<CommEvent> {
        self.comm_manager_tx.clone()
    }

    /// Starts the control thread
    fn control_thread(socket: Socket, handler: Arc<Mutex<dyn ControlHandler>>) {
        let control = Control::new(socket, handler);
        control.listen();
    }

    /// Starts the shell thread.
    fn shell_thread(
        socket: Socket,
        iopub_tx: Sender<IOPubMessage>,
        comm_manager_tx: Sender<CommEvent>,
        comm_changed_rx: Receiver<CommChanged>,
        shell_handler: Arc<Mutex<dyn ShellHandler>>,
        lsp_handler: Option<Arc<Mutex<dyn LspHandler>>>,
    ) -> Result<(), Error> {
        let mut shell = Shell::new(
            socket,
            iopub_tx.clone(),
            comm_manager_tx,
            comm_changed_rx,
            shell_handler,
            lsp_handler,
        );
        shell.listen();
        Ok(())
    }

    /// Starts the IOPub thread.
    fn iopub_thread(socket: Socket, receiver: Receiver<IOPubMessage>) -> Result<(), Error> {
        let mut iopub = IOPub::new(socket, receiver);
        iopub.listen();
        Ok(())
    }

    /// Starts the heartbeat thread.
    fn heartbeat_thread(socket: Socket) -> Result<(), Error> {
        let heartbeat = Heartbeat::new(socket);
        heartbeat.listen();
        Ok(())
    }

    /// Starts the stdin thread.
    fn stdin_thread(
        socket: Socket,
        shell_handler: Arc<Mutex<dyn ShellHandler>>,
    ) -> Result<(), Error> {
        let stdin = Stdin::new(socket, shell_handler);
        stdin.listen();
        Ok(())
    }

    /// Starts the output capture thread.
    fn output_capture_thread(iopub_tx: Sender<IOPubMessage>) -> Result<(), Error> {
        let output_capture = StreamCapture::new(iopub_tx);
        output_capture.listen();
        Ok(())
    }
}
