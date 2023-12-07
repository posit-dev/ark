/*
 * kernel.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Select;
use crossbeam::channel::Sender;
use log::error;
use stdext::spawn;
use stdext::unwrap;

use crate::comm::comm_manager::CommManager;
use crate::comm::event::CommManagerEvent;
use crate::comm::event::CommShellEvent;
use crate::connection_file::ConnectionFile;
use crate::error::Error;
use crate::language::control_handler::ControlHandler;
use crate::language::server_handler::ServerHandler;
use crate::language::shell_handler::ShellHandler;
use crate::session::Session;
use crate::socket::control::Control;
use crate::socket::heartbeat::Heartbeat;
use crate::socket::iopub::IOPub;
use crate::socket::iopub::IOPubMessage;
use crate::socket::shell::Shell;
use crate::socket::socket::Socket;
use crate::socket::stdin::StdInRequest;
use crate::socket::stdin::Stdin;
use crate::stream_capture::StreamCapture;
use crate::wire::input_reply::InputReply;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::OutboundMessage;

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
    comm_manager_tx: Sender<CommManagerEvent>,

    /// Receives notifications about comm changes and events
    comm_manager_rx: Receiver<CommManagerEvent>,
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
        let (comm_manager_tx, comm_manager_rx) = bounded::<CommManagerEvent>(10);

        Ok(Self {
            name: name.to_string(),
            connection: file,
            session: Session::create(key)?,
            iopub_tx,
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
        lsp_handler: Option<Arc<Mutex<dyn ServerHandler>>>,
        dap_handler: Option<Arc<Mutex<dyn ServerHandler>>>,
        stream_behavior: StreamBehavior,
        // Receiver channel for the stdin socket; when input is needed, the
        // language runtime can request it by sending an InputRequest to
        // this channel. The frontend will prompt the user for input and
        // the reply will be delivered via `input_reply_tx`.
        // https://jupyter-client.readthedocs.io/en/stable/messaging.html#messages-on-the-stdin-router-dealer-channel
        stdin_request_rx: Receiver<StdInRequest>,
        // Transmission channel for `input_reply` handling by StdIn
        input_reply_tx: Sender<InputReply>,
    ) -> Result<(), Error> {
        let ctx = zmq::Context::new();

        // Channels for communication of outbound messages between the
        // socket threads and the 0MQ forwarding thread
        let (outbound_tx, outbound_rx) = unbounded();

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
        let dap_handler_clone = dap_handler.clone();
        spawn!(format!("{}-shell", self.name), move || {
            Self::shell_thread(
                shell_socket,
                iopub_tx_clone,
                comm_manager_tx_clone,
                comm_changed_rx,
                shell_clone,
                lsp_handler_clone,
                dap_handler_clone,
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

        let (stdin_inbound_tx, stdin_inbound_rx) = unbounded();
        let (stdin_interrupt_tx, stdin_interrupt_rx) = bounded(1);
        let stdin_session = stdin_socket.session.clone();

        spawn!(format!("{}-stdin", self.name), move || {
            Self::stdin_thread(
                stdin_inbound_rx,
                outbound_tx,
                stdin_request_rx,
                input_reply_tx,
                stdin_interrupt_rx,
                stdin_session,
            )
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

        // Internal sockets for notifying the 0MQ forwarding
        // thread that new outbound messages are available
        let outbound_notif_socket_tx = Socket::new_pair(
            self.session.clone(),
            ctx.clone(),
            String::from("OutboundNotifierTx"),
            None,
            String::from("inproc://outbound_notif"),
            true,
        )?;
        let outbound_notif_socket_rx = Socket::new_pair(
            self.session.clone(),
            ctx.clone(),
            String::from("OutboundNotifierRx"),
            None,
            String::from("inproc://outbound_notif"),
            false,
        )?;

        let outbound_rx_clone = outbound_rx.clone();

        // Forwarding thread that bridges 0MQ sockets and Amalthea
        // channels. Currently only used by StdIn.
        spawn!(format!("{}-zmq-forwarding", self.name), move || {
            Self::zmq_forwarding_thread(
                outbound_notif_socket_rx,
                stdin_socket,
                stdin_inbound_tx,
                outbound_rx_clone,
            )
        });

        // The notifier thread watches Amalthea channels of outgoing
        // messages for readiness. When a channel is hot, it notifies the
        // forwarding thread through a 0MQ socket.
        spawn!(format!("{}-zmq-notifier", self.name), move || {
            Self::zmq_notifier_thread(outbound_notif_socket_tx, outbound_rx)
        });

        let iopub_tx = self.create_iopub_tx();

        spawn!(format!("{}-control", self.name), || {
            Self::control_thread(
                control_socket,
                iopub_tx,
                control_handler,
                stdin_interrupt_tx,
            );
            log::error!("Control thread exited");
        });

        Ok(())
    }

    /// Returns a copy of the IOPub sending channel.
    pub fn create_iopub_tx(&self) -> Sender<IOPubMessage> {
        self.iopub_tx.clone()
    }

    /// Returns a copy of the comm manager sending channel.
    pub fn create_comm_manager_tx(&self) -> Sender<CommManagerEvent> {
        self.comm_manager_tx.clone()
    }

    /// Starts the control thread
    fn control_thread(
        socket: Socket,
        iopub_tx: Sender<IOPubMessage>,
        handler: Arc<Mutex<dyn ControlHandler>>,
        stdin_interrupt_tx: Sender<bool>,
    ) {
        let control = Control::new(socket, iopub_tx, handler, stdin_interrupt_tx);
        control.listen();
    }

    /// Starts the shell thread.
    fn shell_thread(
        socket: Socket,
        iopub_tx: Sender<IOPubMessage>,
        comm_manager_tx: Sender<CommManagerEvent>,
        comm_changed_rx: Receiver<CommShellEvent>,
        shell_handler: Arc<Mutex<dyn ShellHandler>>,
        lsp_handler: Option<Arc<Mutex<dyn ServerHandler>>>,
        dap_handler: Option<Arc<Mutex<dyn ServerHandler>>>,
    ) -> Result<(), Error> {
        let mut shell = Shell::new(
            socket,
            iopub_tx.clone(),
            comm_manager_tx,
            comm_changed_rx,
            shell_handler,
            lsp_handler,
            dap_handler,
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
        inbound_rx: Receiver<Message>,
        outbound_tx: Sender<OutboundMessage>,
        stdin_request_rx: Receiver<StdInRequest>,
        input_reply_tx: Sender<InputReply>,
        interrupt_rx: Receiver<bool>,
        session: Session,
    ) -> Result<(), Error> {
        let stdin = Stdin::new(inbound_rx, outbound_tx, session);
        stdin.listen(stdin_request_rx, input_reply_tx, interrupt_rx);
        Ok(())
    }

    /// Starts the thread that forwards 0MQ messages to Amalthea channels
    /// and vice versa.
    fn zmq_forwarding_thread(
        outbound_notif_socket: Socket,
        stdin_socket: Socket,
        stdin_inbound_tx: Sender<Message>,
        outbound_rx: Receiver<OutboundMessage>,
    ) {
        // This function checks for notifications that an outgoing message
        // is ready to be read on an Amalthea channel. It returns
        // immediately whether a message is ready or not.
        let has_outbound = || -> bool {
            if let Ok(n) = outbound_notif_socket.socket.poll(zmq::POLLIN, 0) {
                if n == 0 {
                    return false;
                }
                // Consume notification
                let _ = unwrap!(outbound_notif_socket.socket.recv_bytes(0), Err(err) => {
                    log::error!("Could not consume outbound notification socket: {}", err);
                    return false;
                });

                true
            } else {
                false
            }
        };

        // This function checks that a 0MQ message from the frontend is ready.
        let has_inbound = || -> bool {
            match stdin_socket.socket.poll(zmq::POLLIN, 0) {
                Ok(n) if n > 0 => true,
                _ => false,
            }
        };

        // Forwards channel message from Amalthea to the frontend via the
        // corresponding 0MQ socket. Should consume exactly 1 message and
        // notify back the notifier thread to keep the mechanism synchronised.
        let forward_outbound = || -> anyhow::Result<()> {
            // Consume message and forward it
            let outbound_msg = outbound_rx.recv()?;
            match outbound_msg {
                OutboundMessage::StdIn(msg) => msg.send(&stdin_socket)?,
            };

            // Notify back
            outbound_notif_socket.send(zmq::Message::new())?;

            Ok(())
        };

        // Forwards 0MQ message from the frontend to the corresponding
        // Amalthea channel.
        let forward_inbound = || -> anyhow::Result<()> {
            let msg = Message::read_from_socket(&stdin_socket)?;
            stdin_inbound_tx.send(msg)?;
            Ok(())
        };

        // Create poll items necessary to call `zmq_poll()`
        let mut poll_items = {
            let outbound_notif_poll_item = outbound_notif_socket.socket.as_poll_item(zmq::POLLIN);
            let stdin_poll_item = stdin_socket.socket.as_poll_item(zmq::POLLIN);
            vec![outbound_notif_poll_item, stdin_poll_item]
        };

        loop {
            let n = unwrap!(
                zmq::poll(&mut poll_items, -1),
                Err(err) => {
                    error!("While polling 0MQ items: {}", err);
                    0
                }
            );

            for _ in 0..n {
                if has_outbound() {
                    unwrap!(
                        forward_outbound(),
                        Err(err) => error!("While forwarding outbound message: {}", err)
                    );
                    continue;
                }

                if has_inbound() {
                    unwrap!(
                        forward_inbound(),
                        Err(err) => error!("While forwarding inbound message: {}", err)
                    );
                    continue;
                }

                log::error!("Could not find readable message");
            }
        }
    }

    /// Starts the thread that notifies the forwarding thread that new
    /// outgoing messages have arrived from Amalthea.
    fn zmq_notifier_thread(notif_socket: Socket, outbound_rx: Receiver<OutboundMessage>) {
        let mut sel = Select::new();
        sel.recv(&outbound_rx);

        loop {
            let _ = sel.ready();

            unwrap!(
                notif_socket.send(zmq::Message::new()),
                Err(err) => {
                    error!("Couldn't notify 0MQ thread: {}", err);
                    continue;
                }
            );

            // To keep things synchronised, wait to be notified that the
            // channel message has been consumed before continuing the loop.
            unwrap!(
                {
                    let mut msg = zmq::Message::new();
                    notif_socket.recv(&mut msg)
                },
                Err(err) => {
                    error!("Couldn't received acknowledgement from 0MQ thread: {}", err);
                    continue;
                }
            );
        }
    }

    /// Starts the output capture thread.
    fn output_capture_thread(iopub_tx: Sender<IOPubMessage>) -> Result<(), Error> {
        let output_capture = StreamCapture::new(iopub_tx);
        output_capture.listen();
        Ok(())
    }
}
