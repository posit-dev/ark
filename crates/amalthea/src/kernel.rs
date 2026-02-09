/*
 * kernel.rs
 *
 * Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
 *
 */

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use stdext::debug_panic;
use stdext::spawn;
use stdext::unwrap;

use crate::comm::event::CommManagerEvent;
use crate::connection_file::ConnectionFile;
use crate::error::Error;
use crate::language::control_handler::ControlHandler;
use crate::language::server_handler::ServerHandler;
use crate::language::shell_handler::ShellHandler;
use crate::registration_file::RegistrationFile;
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
use crate::wire::handshake_request::HandshakeRequest;
use crate::wire::input_reply::InputReply;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::OutboundMessage;
use crate::wire::jupyter_message::Status;
use crate::wire::subscription_message::SubscriptionMessage;

/// Possible behaviors for the stream capture thread. When set to `Capture`,
/// the stream capture thread will capture all output to stdout and stderr.
/// When set to `None`, no stream output is captured.
#[derive(PartialEq)]
pub enum StreamBehavior {
    Capture,
    None,
}

/// Connects the Kernel to the frontend
pub fn connect(
    name: &str,
    connection_file: ConnectionFile,
    registration_file: Option<RegistrationFile>,
    shell_handler: Box<dyn ShellHandler>,
    control_handler: Arc<Mutex<dyn ControlHandler>>,
    server_handlers: HashMap<String, Arc<Mutex<dyn ServerHandler>>>,
    stream_behavior: StreamBehavior,
    iopub_tx: Sender<IOPubMessage>,
    iopub_rx: Receiver<IOPubMessage>,
    comm_manager_rx: Receiver<CommManagerEvent>,
    // Receiver channel for the stdin socket; when input is needed, the
    // language runtime can request it by sending an StdInRequest::Input to
    // this channel. The frontend will prompt the user for input and
    // the reply will be delivered via `stdin_reply_tx`.
    // https://jupyter-client.readthedocs.io/en/stable/messaging.html#messages-on-the-stdin-router-dealer-channel.
    // Note that we've extended the StdIn socket to support synchronous requests
    // from a comm, see `StdInRequest::Comm`.
    stdin_request_rx: Receiver<StdInRequest>,
    // Transmission channel for StdIn replies
    stdin_reply_tx: Sender<crate::Result<InputReply>>,
) -> Result<(), Error> {
    let ctx = zmq::Context::new();

    let session = Session::create(connection_file.key.as_str())?;

    // Channels for communication of outbound messages between the
    // socket threads and the 0MQ forwarding thread
    let (outbound_tx, outbound_rx) = unbounded();

    // Create the Shell ROUTER/DEALER socket and start a thread to listen
    // for client messages.
    let shell_socket = Socket::new(
        session.clone(),
        ctx.clone(),
        String::from("Shell"),
        zmq::ROUTER,
        None,
        connection_file.endpoint(connection_file.shell_port),
    )?;
    let shell_port = port_finalize(&shell_socket, connection_file.shell_port)?;

    // Internal sockets for notifying Shell when comm events arrive
    let notif_endpoint = String::from("inproc://shell_comm_notifier");
    let shell_comm_notif_socket_tx = Socket::new_pair(
        session.clone(),
        ctx.clone(),
        String::from("ShellCommNotifierTx"),
        None,
        notif_endpoint.clone(),
        true,
    )?;
    let shell_comm_notif_socket_rx = Socket::new_pair(
        session.clone(),
        ctx.clone(),
        String::from("ShellCommNotifierRx"),
        None,
        notif_endpoint,
        false,
    )?;

    // Channel for comm events flowing from notifier thread to Shell. The
    // notifier watches `comm_manager_rx` and forwards events via `shell_comm_tx`.
    let (shell_comm_tx, shell_comm_rx) = unbounded::<CommManagerEvent>();

    let iopub_tx_clone = iopub_tx.clone();
    spawn!(format!("{name}-shell"), move || {
        shell_thread(
            shell_socket,
            iopub_tx_clone,
            shell_comm_notif_socket_rx,
            shell_comm_rx,
            shell_handler,
            server_handlers,
        )
    });

    // Create the IOPub XPUB/SUB socket and start a thread to broadcast to
    // the client. IOPub only broadcasts messages, so it listens to other
    // threads on a Receiver<Message> instead of to the client.
    let iopub_socket = Socket::new(
        session.clone(),
        ctx.clone(),
        String::from("IOPub"),
        zmq::XPUB,
        None,
        connection_file.endpoint(connection_file.iopub_port),
    )?;
    let iopub_port = port_finalize(&iopub_socket, connection_file.iopub_port)?;

    let (iopub_inbound_tx, iopub_inbound_rx) = unbounded();
    let iopub_session = iopub_socket.session.clone();
    let iopub_outbound_tx = outbound_tx.clone();

    // Channel used for notifying back that the XPUB socket for IOPub has received a
    // subscription message, meaning the messages we send over IOPub will no longer be
    // dropped by our socket on the way out.
    let (iopub_subscription_tx, iopub_subscription_rx) = bounded::<()>(1);

    spawn!(format!("{name}-iopub"), move || {
        iopub_thread(
            iopub_rx,
            iopub_inbound_rx,
            iopub_outbound_tx,
            iopub_subscription_tx,
            iopub_session,
        )
    });

    // Create the heartbeat socket and start a thread to listen for
    // heartbeat messages.
    let heartbeat_socket = Socket::new(
        session.clone(),
        ctx.clone(),
        String::from("Heartbeat"),
        zmq::REP,
        None,
        connection_file.endpoint(connection_file.hb_port),
    )?;
    let hb_port = port_finalize(&heartbeat_socket, connection_file.hb_port)?;
    spawn!(format!("{name}-heartbeat"), move || {
        heartbeat_thread(heartbeat_socket)
    });

    // Create the stdin socket and start a thread to listen for stdin
    // messages. These are used by the kernel to request input from the
    // user, and so flow in the opposite direction to the other sockets.
    let stdin_socket = Socket::new(
        session.clone(),
        ctx.clone(),
        String::from("Stdin"),
        zmq::ROUTER,
        None,
        connection_file.endpoint(connection_file.stdin_port),
    )?;
    let stdin_port = port_finalize(&stdin_socket, connection_file.stdin_port)?;

    let (stdin_inbound_tx, stdin_inbound_rx) = unbounded();
    let (stdin_interrupt_tx, stdin_interrupt_rx) = bounded(1);
    let stdin_session = stdin_socket.session.clone();
    let stdin_outbound_tx = outbound_tx.clone();

    spawn!(format!("{name}-stdin"), move || {
        stdin_thread(
            stdin_inbound_rx,
            stdin_outbound_tx,
            stdin_request_rx,
            stdin_reply_tx,
            stdin_interrupt_rx,
            stdin_session,
        )
    });

    // Create the thread that handles stdout and stderr, if requested
    if stream_behavior == StreamBehavior::Capture {
        let iopub_tx_clone = iopub_tx.clone();
        spawn!(format!("{name}-output-capture"), move || {
            output_capture_thread(iopub_tx_clone)
        });
    }

    // Create the Control ROUTER/DEALER socket
    let control_socket = Socket::new(
        session.clone(),
        ctx.clone(),
        String::from("Control"),
        zmq::ROUTER,
        None,
        connection_file.endpoint(connection_file.control_port),
    )?;
    let control_port = port_finalize(&control_socket, connection_file.control_port)?;

    // Internal sockets for notifying the 0MQ forwarding
    // thread that new outbound messages are available
    let outbound_notif_socket_tx = Socket::new_pair(
        session.clone(),
        ctx.clone(),
        String::from("OutboundNotifierTx"),
        None,
        String::from("inproc://outbound_notif"),
        true,
    )?;
    let outbound_notif_socket_rx = Socket::new_pair(
        session.clone(),
        ctx.clone(),
        String::from("OutboundNotifierRx"),
        None,
        String::from("inproc://outbound_notif"),
        false,
    )?;

    // Channel for outbound messages flowing from notifier to ZMQ forwarding thread.
    // The notifier watches `outbound_rx` and forwards messages via `zmq_outbound_tx`.
    let (zmq_outbound_tx, zmq_outbound_rx) = unbounded::<OutboundMessage>();

    // Socket bridge thread: owns the external ZMQ sockets (IOPub, StdIn) and
    // bridges them to/from Amalthea channels. Potentially all the sockets
    // could live there. That would allow consistent channel messaging
    // throughout Amalthea.
    spawn!(format!("{name}-socket-bridge"), move || {
        socket_bridge_thread(
            outbound_notif_socket_rx,
            stdin_socket,
            stdin_inbound_tx,
            iopub_socket,
            iopub_inbound_tx,
            zmq_outbound_rx,
        )
    });

    // Channel bridge thread: watches crossbeam channels and makes them pollable
    // via inproc ZMQ sockets. This allows threads to use `zmq_poll()` to wait on
    // both external ZMQ sockets and internal channel events.
    // - outbound_rx -> socket_bridge_thread (for IOPub/StdIn messages)
    // - comm_manager_rx -> Shell (for comm events from backend)
    spawn!(format!("{name}-channel-bridge"), move || {
        channel_bridge_thread(
            outbound_notif_socket_tx,
            outbound_rx,
            zmq_outbound_tx,
            shell_comm_notif_socket_tx,
            comm_manager_rx,
            shell_comm_tx,
        )
    });

    let iopub_tx_clone = iopub_tx.clone();

    spawn!(format!("{name}-control"), || {
        control_thread(
            control_socket,
            iopub_tx_clone,
            control_handler,
            stdin_interrupt_tx,
        );
        log::error!("Control thread exited");
    });

    if let Some(registration_file) = registration_file {
        handshake(
            registration_file,
            &ctx,
            &session,
            control_port,
            shell_port,
            stdin_port,
            iopub_port,
            hb_port,
        )?;
    };

    // Wait until we have our first (and usually only) IOPub subscription message come in.
    // This means that someone is actually connected on the other side. Without a
    // subscriber, the IOPub socket will simply drop any critical messages we try and send
    // out too early (which can happen with stdout emitted from `.Rprofile`, or busy/idle
    // messages that are sent very early on). Even the handshake above isn't a replacement
    // for this. The `HandshakeReply` ensures that the client has received our port
    // numbers, but does not ensure that the client's IOPub socket has connected or
    // subscribed.
    log::info!("Waiting on IOPub subscription confirmation");
    match iopub_subscription_rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(_) => {
            log::info!("Received IOPub subscription confirmation, completing kernel connection");
        },
        Err(err) => {
            panic!("Failed to receive IOPub subscription confirmation. Aborting. Error: {err:?}");
        },
    }

    Ok(())
}

/// Reads a `connection_file` containing Jupyter connection information
///
/// Most frontends will provide a `connection_file` specifying their socket ports.
/// This reads directly into a fully fleshed out `ConnectionFile`.
/// However, this has a well known race condition where the Client selects the
/// ports, but the Server binds to them, and someone else could take the ports in
/// the time between the Client picks them and the Server binds.
///
/// To avoid this, we provide an alternative method of connection through a `RegistrationFile`.
/// This specifies a `registration_port` that the Client has bound to, which we will send
/// the remaining port informtation back to after we have bound to the ports ourselves.
/// The `ConnectionFile` we return in this case temporarily has `0`s as the port numbers,
/// which tells zeromq to bind to whatever random port the OS sees as free.
///
/// See https://github.com/jupyter/enhancement-proposals/pull/66.
pub fn read_connection(connection_file: &str) -> (ConnectionFile, Option<RegistrationFile>) {
    match ConnectionFile::from_file(connection_file) {
        Ok(connection) => {
            log::info!("Loaded connection information from frontend in {connection_file}");
            log::info!("Connection data: {connection:?}");
            return (connection, None);
        },
        Err(err) => {
            log::info!(
                "Failed to load `ConnectionFile`, trying to load as `RegistrationFile` instead:\n{err:?}"
            );
        },
    }

    match RegistrationFile::from_file(connection_file) {
        Ok(registration) => {
            log::info!("Loaded registration information from frontend in {connection_file}");
            log::info!("Registration data: {registration:?}");
            let connection = registration.as_connection_file();
            return (connection, Some(registration));
        },
        Err(err) => {
            panic!("Failed to load `connection_file` as both `ConnectionFile` and `RegistrationFile`:\n{err:?}")
        },
    }
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
    comm_notif_socket: Socket,
    comm_manager_rx: Receiver<CommManagerEvent>,
    shell_handler: Box<dyn ShellHandler>,
    server_handlers: HashMap<String, Arc<Mutex<dyn ServerHandler>>>,
) -> Result<(), Error> {
    let mut shell = Shell::new(
        socket,
        iopub_tx.clone(),
        comm_notif_socket,
        comm_manager_rx,
        shell_handler,
        server_handlers,
    );
    shell.listen();
    Ok(())
}

/// Starts the IOPub thread.
fn iopub_thread(
    rx: Receiver<IOPubMessage>,
    inbound_rx: Receiver<crate::Result<SubscriptionMessage>>,
    outbound_tx: Sender<OutboundMessage>,
    subscription_tx: Sender<()>,
    session: Session,
) -> Result<(), Error> {
    let mut iopub = IOPub::new(rx, inbound_rx, outbound_tx, subscription_tx, session);
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
    inbound_rx: Receiver<crate::Result<Message>>,
    outbound_tx: Sender<OutboundMessage>,
    stdin_request_rx: Receiver<StdInRequest>,
    stdin_reply_tx: Sender<crate::Result<InputReply>>,
    interrupt_rx: Receiver<bool>,
    session: Session,
) -> Result<(), Error> {
    let stdin = Stdin::new(inbound_rx, outbound_tx, session);
    stdin.listen(stdin_request_rx, stdin_reply_tx, interrupt_rx);
    Ok(())
}

/// Socket bridge: owns external ZMQ sockets (IOPub, StdIn) and bridges them
/// to/from Amalthea channels.
///
/// This solves the problem of polling/selecting from ZMQ sockets and crossbeam
/// channels at the same time. ZMQ sockets can only be owned by one thread, but
/// we need to listen for multiple event sources. For example, with IOPub we need
/// to both send messages to the frontend AND listen for subscription events.
///
/// The solution: the channel bridge thread watches crossbeam channels and forwards
/// messages to this thread via `zmq_outbound_rx`, then sends a ZMQ notification.
/// This thread then uses `zmq_poll()` to wait on:
/// - Outbound notification socket: wakes up when messages are ready to send
/// - StdIn socket: receives replies from the frontend
/// - IOPub socket: receives subscription events from the frontend
///
/// When the outbound notification fires, this thread drains all pending messages
/// from `zmq_outbound_rx` and sends them to the appropriate ZMQ socket.
///
/// Terminology:
/// - Outbound: Amalthea channel -> ZMQ socket (e.g. IOPub messages to frontend)
/// - Inbound: ZMQ socket -> Amalthea channel (e.g. StdIn replies from frontend)
fn socket_bridge_thread(
    outbound_notif_socket: Socket,
    stdin_socket: Socket,
    stdin_inbound_tx: Sender<crate::Result<Message>>,
    iopub_socket: Socket,
    iopub_inbound_tx: Sender<crate::Result<SubscriptionMessage>>,
    zmq_outbound_rx: Receiver<OutboundMessage>,
) {
    // Consume notification and return whether one was present.
    let consume_outbound_notification = || -> bool {
        if let Ok(n) = outbound_notif_socket.socket.poll(zmq::POLLIN, 0) {
            if n == 0 {
                return false;
            }
            // Consume notification
            if let Err(err) = outbound_notif_socket.socket.recv_bytes(0) {
                debug_panic!("Could not consume outbound notification: {err:?}");
                return false;
            }
            true
        } else {
            false
        }
    };

    // This function checks that a 0MQ message from the frontend is ready.
    let has_inbound = |socket: &Socket| -> bool {
        match socket.socket.poll(zmq::POLLIN, 0) {
            Ok(n) if n > 0 => true,
            _ => false,
        }
    };

    // Drain all pending outbound messages and forward to ZMQ.
    let drain_outbound = || {
        while let Ok(outbound_msg) = zmq_outbound_rx.try_recv() {
            let result = match outbound_msg {
                OutboundMessage::StdIn(msg) => msg.send(&stdin_socket),
                OutboundMessage::IOPub(msg) => msg.send(&iopub_socket),
            };
            if let Err(err) = result {
                debug_panic!("While forwarding outbound message: {err:?}");
            }
        }
    };

    // Forwards 0MQ message from the frontend to the corresponding
    // Amalthea channel.
    let forward_inbound =
        |socket: &Socket, inbound_tx: &Sender<crate::Result<Message>>| -> anyhow::Result<()> {
            let msg = Message::read_from_socket(socket);
            inbound_tx.send(msg)?;
            Ok(())
        };

    // Forwards special 0MQ XPUB subscription message from the frontend to the IOPub thread.
    let forward_inbound_subscription = |socket: &Socket,
                                        inbound_tx: &Sender<crate::Result<SubscriptionMessage>>|
     -> anyhow::Result<()> {
        let msg = SubscriptionMessage::read_from_socket(socket);
        inbound_tx.send(msg)?;
        Ok(())
    };

    // Create poll items necessary to call `zmq_poll()`
    let mut poll_items = {
        let outbound_notif_poll_item = outbound_notif_socket.socket.as_poll_item(zmq::POLLIN);
        let stdin_poll_item = stdin_socket.socket.as_poll_item(zmq::POLLIN);
        let iopub_poll_item = iopub_socket.socket.as_poll_item(zmq::POLLIN);
        vec![outbound_notif_poll_item, stdin_poll_item, iopub_poll_item]
    };

    loop {
        let n = unwrap!(
            zmq::poll(&mut poll_items, -1),
            Err(err) => {
                debug_panic!("While polling 0MQ items: {err:?}");
                0
            }
        );

        for _ in 0..n {
            if consume_outbound_notification() {
                drain_outbound();
                continue;
            }

            if has_inbound(&stdin_socket) {
                unwrap!(
                    forward_inbound(&stdin_socket, &stdin_inbound_tx),
                    Err(err) => debug_panic!("While forwarding inbound message: {err:?}")
                );
                continue;
            }

            if has_inbound(&iopub_socket) {
                unwrap!(
                    forward_inbound_subscription(&iopub_socket, &iopub_inbound_tx),
                    Err(err) => debug_panic!("While forwarding inbound message: {err:?}")
                );
                continue;
            }

            debug_panic!("Could not find readable message");
        }
    }
}

/// A channel forwarder that consumes from a source channel, forwards to a
/// destination channel, and sends a ZMQ notification so the destination wakes
/// up from watching ZMQ socket to inspect the Crossbeam channels.
struct Forwarder<T> {
    name: &'static str,
    source_rx: Receiver<T>,
    destination_tx: Sender<T>,
    notif_socket: Socket,
    connected: bool,
}

impl<T> Forwarder<T> {
    fn new(
        name: &'static str,
        source_rx: Receiver<T>,
        destination_tx: Sender<T>,
        notif_socket: Socket,
    ) -> Self {
        Self {
            name,
            source_rx,
            destination_tx,
            notif_socket,
            connected: true,
        }
    }

    /// Process a ready notification: consume, forward, and notify.
    fn process(&mut self) {
        // Consume from source
        let msg = match self.source_rx.try_recv() {
            Ok(msg) => msg,
            Err(crossbeam::channel::TryRecvError::Empty) => return,
            Err(crossbeam::channel::TryRecvError::Disconnected) => {
                debug_panic!("{} channel disconnected", self.name);
                self.connected = false;
                return;
            },
        };

        // Forward to destination
        if let Err(err) = self.destination_tx.send(msg) {
            debug_panic!("Couldn't forward {} message: {err:?}", self.name);
            self.connected = false;
            return;
        }

        // Notify destination via inproc PAIR socket
        if let Err(err) = self.notif_socket.socket.send(zmq::Message::new(), 0) {
            debug_panic!("Couldn't send {} notification: {err:?}", self.name);
        }
    }
}

/// Channel bridge: watches crossbeam channels and makes them pollable via
/// inproc ZMQ sockets.
///
/// This thread bridges crossbeam channels to ZMQ poll() by:
/// 1. Watching `outbound_rx` for IOPub/StdIn messages -> forwards via `zmq_outbound_tx`
/// 2. Watching `comm_manager_rx` for comm events -> forwards via `shell_comm_tx`
///
/// Both use fire-and-forget notifications: consumes messages, forwards them
/// through a channel, and sends a DONTWAIT notification. Consumer threads
/// drain all pending messages when they wake up.
fn channel_bridge_thread(
    outbound_notif_socket: Socket,
    outbound_rx: Receiver<OutboundMessage>,
    zmq_outbound_tx: Sender<OutboundMessage>,
    shell_comm_notif_socket: Socket,
    comm_manager_rx: Receiver<CommManagerEvent>,
    shell_comm_tx: Sender<CommManagerEvent>,
) {
    use crossbeam::channel::Select;

    let mut outbound = Forwarder::new(
        "outbound",
        outbound_rx,
        zmq_outbound_tx,
        outbound_notif_socket,
    );
    let mut comm = Forwarder::new(
        "comm",
        comm_manager_rx,
        shell_comm_tx,
        shell_comm_notif_socket,
    );

    loop {
        let mut sel = Select::new();
        let outbound_idx = if outbound.connected {
            Some(sel.recv(&outbound.source_rx))
        } else {
            None
        };
        let comm_idx = if comm.connected {
            Some(sel.recv(&comm.source_rx))
        } else {
            None
        };

        if outbound_idx.is_none() && comm_idx.is_none() {
            log::info!("All channels disconnected, notifier thread exiting");
            return;
        }

        // Block until one channel is ready
        let ready_idx = sel.ready();

        if outbound_idx.is_some_and(|idx| ready_idx == idx) {
            outbound.process();
            continue;
        }

        if comm_idx.is_some_and(|idx| ready_idx == idx) {
            comm.process();
            continue;
        }
    }
}

/// Starts the output capture thread.
fn output_capture_thread(iopub_tx: Sender<IOPubMessage>) -> Result<(), Error> {
    let output_capture = StreamCapture::new(iopub_tx);
    output_capture.listen();
    Ok(())
}

fn handshake(
    registration_file: RegistrationFile,
    ctx: &zmq::Context,
    session: &Session,
    control_port: u16,
    shell_port: u16,
    stdin_port: u16,
    iopub_port: u16,
    hb_port: u16,
) -> crate::Result<()> {
    // Create a temporary registration socket to send the handshake request over.
    // This socket `Drop`s and closes when this function exits.
    let registration_socket = Socket::new(
        session.clone(),
        ctx.clone(),
        String::from("Registration"),
        zmq::REQ,
        None,
        registration_file.endpoint(),
    )?;

    let message = HandshakeRequest {
        control_port,
        shell_port,
        stdin_port,
        iopub_port,
        hb_port,
    };
    let message = JupyterMessage::create(message, None, &session);

    message.send(&registration_socket)?;

    // Wait for the handshake reply with a 5 second timeout.
    // If we don't get a handshake reply, we are going to eventually panic and shut down.
    if !registration_socket
        .poll_incoming(5000)
        .map_err(|err| Error::ZmqError(registration_socket.name.clone(), err))?
    {
        return Err(crate::anyhow!(
            "Timeout while waiting for connection information from registration socket"
        ));
    }

    // Read the `HandshakeReply` off the socket and confirm its message type
    let reply = Message::read_from_socket(&registration_socket).unwrap();
    let status = match reply {
        Message::HandshakeReply(reply) => reply.content.status,
        _ => {
            return Err(crate::anyhow!(
                "Unexpected message type received from registration socket: {reply:?}"
            ));
        },
    };

    // Check that the client did indeed connect successfully
    match status {
        Status::Ok => Ok(()),
        Status::Error => Err(crate::anyhow!("Client failed to connect to ports.")),
    }
}

fn port_finalize(socket: &Socket, port: u16) -> crate::Result<u16> {
    if port == 0 {
        // Server provided the port, extract it from the socket
        // since we gave zmq a port number of `0` to begin with.
        return port_from_socket(socket);
    } else {
        // Client provided the port, just use that
        return Ok(port);
    }
}

pub(crate) fn port_from_socket(socket: &Socket) -> crate::Result<u16> {
    let name = socket.name.as_str();

    let address = match socket.socket.get_last_endpoint() {
        Ok(address) => address,
        Err(err) => {
            return Err(crate::anyhow!(
                "Can't access last endpoint of '{name}' socket due to {err:?}"
            ));
        },
    };

    let address = match address {
        Ok(address) => address,
        Err(_) => {
            return Err(crate::anyhow!(
                "Can't access last endpoint of '{name}' socket."
            ));
        },
    };

    // We've got the full address but we only want the port at the very end
    let Some(loc) = address.rfind(":") else {
        return Err(crate::anyhow!(
            "Failed to find port in the '{name}' socket address."
        ));
    };

    let port = &address[(loc + 1)..];

    let port = match port.parse::<u16>() {
        Ok(port) => port,
        Err(err) => {
            return Err(crate::anyhow!(
                "Can't parse port '{port}' into a `u16` due to {err:?}"
            ));
        },
    };

    Ok(port)
}
