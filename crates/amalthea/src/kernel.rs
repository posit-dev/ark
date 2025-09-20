/*
 * kernel.rs
 *
 * Copyright (C) 2022-2025 Posit Software, PBC. All rights reserved.
 *
 */

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Select;
use crossbeam::channel::Sender;
use stdext::spawn;
use stdext::unwrap;

use crate::comm::comm_manager::CommManager;
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

macro_rules! report_error {
    ($($arg:tt)+) => (if cfg!(debug_assertions) { panic!($($arg)+) } else { log::error!($($arg)+) })
}

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
    comm_manager_tx: Sender<CommManagerEvent>,
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

    // Create the comm manager thread
    CommManager::start(iopub_tx.clone(), comm_manager_rx);

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

    let iopub_tx_clone = iopub_tx.clone();
    spawn!(format!("{name}-shell"), move || {
        shell_thread(
            shell_socket,
            iopub_tx_clone,
            comm_manager_tx,
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

    let outbound_rx_clone = outbound_rx.clone();

    // Forwarding thread that bridges 0MQ sockets and Amalthea
    // channels. Currently only used by StdIn.
    spawn!(format!("{name}-zmq-forwarding"), move || {
        zmq_forwarding_thread(
            outbound_notif_socket_rx,
            stdin_socket,
            stdin_inbound_tx,
            iopub_socket,
            iopub_inbound_tx,
            outbound_rx_clone,
        )
    });

    // The notifier thread watches Amalthea channels of outgoing
    // messages for readiness. When a channel is hot, it notifies the
    // forwarding thread through a 0MQ socket.
    spawn!(format!("{name}-zmq-notifier"), move || {
        zmq_notifier_thread(outbound_notif_socket_tx, outbound_rx)
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
    comm_manager_tx: Sender<CommManagerEvent>,
    shell_handler: Box<dyn ShellHandler>,
    server_handlers: HashMap<String, Arc<Mutex<dyn ServerHandler>>>,
) -> Result<(), Error> {
    let mut shell = Shell::new(
        socket,
        iopub_tx.clone(),
        comm_manager_tx,
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

/// Starts the thread that forwards 0MQ messages to Amalthea channels
/// and vice versa.
///
/// This is a solution to the problem of polling/selecting from 0MQ sockets and
/// crossbeam channels at the same time. Message events on crossbeam channels
/// are emitted by the notifier thread (see below) on a 0MQ socket. The
/// forwarding thread is then able to listen on 0MQ sockets (e.g. StdIn replies
/// and IOPub subscriptions) and the notification socket at the same time.
///
/// Part of the problem this setup solves is that 0MQ sockets can only be owned
/// by one thread at a time. Take IOPUb as an example: we need to listen on that
/// socket for subscription events. We also need to listen for new IOPub
/// messages to send to the client, sent via Crossbeam channels. So we need at
/// least two threads listening for these two different kinds of events. But the
/// forwarding thread has to fully own the socket to be able to listen to it. So
/// it's also in charge of sending IOPub messages on that socket. When an IOPub
/// message comes in, the notifier thread wakes up the forwarding thread which
/// then pulls messages from the channel and forwards them to the IOPub socket.
///
/// Terminology:
/// - Outbound means that a crossbeam message needs to be forwarded to a 0MQ socket.
/// - Inbound means that a 0MQ message needs to be forwarded to a crossbeam channel.
fn zmq_forwarding_thread(
    outbound_notif_socket: Socket,
    stdin_socket: Socket,
    stdin_inbound_tx: Sender<crate::Result<Message>>,
    iopub_socket: Socket,
    iopub_inbound_tx: Sender<crate::Result<SubscriptionMessage>>,
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
                report_error!("Could not consume outbound notification socket: {}", err);
                return false;
            });

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

    // Forwards channel message from Amalthea to the frontend via the
    // corresponding 0MQ socket. Should consume exactly 1 message and
    // notify back the notifier thread to keep the mechanism synchronised.
    let forward_outbound = || -> anyhow::Result<()> {
        // Consume message and forward it
        let outbound_msg = outbound_rx.recv()?;
        match outbound_msg {
            OutboundMessage::StdIn(msg) => msg.send(&stdin_socket)?,
            OutboundMessage::IOPub(msg) => msg.send(&iopub_socket)?,
        };

        // Notify back
        outbound_notif_socket.send(zmq::Message::new())?;

        Ok(())
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
                report_error!("While polling 0MQ items: {}", err);
                0
            }
        );

        for _ in 0..n {
            if has_outbound() {
                unwrap!(
                    forward_outbound(),
                    Err(err) => report_error!("While forwarding outbound message: {}", err)
                );
                continue;
            }

            if has_inbound(&stdin_socket) {
                unwrap!(
                    forward_inbound(&stdin_socket, &stdin_inbound_tx),
                    Err(err) => report_error!("While forwarding inbound message: {}", err)
                );
                continue;
            }

            if has_inbound(&iopub_socket) {
                unwrap!(
                    forward_inbound_subscription(&iopub_socket, &iopub_inbound_tx),
                    Err(err) => report_error!("While forwarding inbound message: {}", err)
                );
                continue;
            }

            report_error!("Could not find readable message");
        }
    }
}

/// Starts the thread that notifies the forwarding thread that new outgoing
/// messages have arrived from Amalthea channels. This wakes up the forwarding
/// thread which will then pop the message from the channel and forward them to
/// the relevant zeromq socket.
fn zmq_notifier_thread(notif_socket: Socket, outbound_rx: Receiver<OutboundMessage>) {
    let mut sel = Select::new();
    sel.recv(&outbound_rx);

    loop {
        let _ = sel.ready();

        unwrap!(
            notif_socket.send(zmq::Message::new()),
            Err(err) => {
                report_error!("Couldn't notify 0MQ thread: {}", err);
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
                report_error!("Couldn't received acknowledgement from 0MQ thread: {}", err);
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
