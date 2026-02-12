/*
 * shell.rs
 *
 * Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
 *
 */

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use futures::executor::block_on;
use stdext::result::ResultExt;

use crate::comm::comm_channel::comm_rpc_message;
use crate::comm::comm_channel::Comm;
use crate::comm::comm_channel::CommMsg;
use crate::comm::event::CommEvent;
use crate::comm::server_comm::ServerComm;
use crate::comm::server_comm::ServerStartedMessage;
use crate::error::Error;
use crate::language::server_handler::ServerHandler;
use crate::language::shell_handler::ShellHandler;
use crate::socket::comm::CommInitiator;
use crate::socket::comm::CommSocket;
use crate::socket::iopub::IOPubContextChannel;
use crate::socket::iopub::IOPubMessage;
use crate::socket::socket::Socket;
use crate::wire::comm_close::CommClose;
use crate::wire::comm_info_reply::CommInfoReply;
use crate::wire::comm_info_reply::CommInfoTargetName;
use crate::wire::comm_info_request::CommInfoRequest;
use crate::wire::comm_msg::CommWireMsg;
use crate::wire::comm_open::CommOpen;
use crate::wire::exception::Exception;
use crate::wire::header::JupyterHeader;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::jupyter_message::Status;
use crate::wire::kernel_info_full_reply;
use crate::wire::originator::Originator;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;

/// Wrapper for the Shell socket; receives requests for execution, etc. from the
/// frontend and handles them or dispatches them to the execution thread.
///
/// Shell also manages comm channels (Jupyter's bidirectional communication
/// mechanism for custom messages between frontend and backend). This includes:
/// - Handling `comm_open`, `comm_msg`, and `comm_close` requests from the frontend
/// - Processing backend-initiated comm events (opens, messages) via `comm_manager_rx`
/// - Routing messages to/from individual comm handlers
///
/// Comm management lives in Shell because frontend comm requests (`comm_open`,
/// `comm_msg`, `comm_close`) arrive on the Shell socket. Backend-initiated comms
/// piggyback on this infrastructure via `comm_manager_rx`.
pub struct Shell {
    /// The ZeroMQ Shell socket
    socket: Socket,

    /// Sends messages to the IOPub socket (owned by another thread)
    iopub_tx: Sender<IOPubMessage>,

    /// Language-provided shell handler object
    shell_handler: Box<dyn ShellHandler>,

    /// Map of server handler target names to their handlers
    server_handlers: HashMap<String, Arc<Mutex<dyn ServerHandler>>>,

    /// Socket to receive notifications when comm events arrive
    comm_notif_socket: Socket,

    /// Channel to receive comm registration events from backend-initiated comms
    comm_event_rx: Receiver<CommEvent>,

    /// The set of currently open comm channels
    open_comms: Vec<CommSocket>,
}

impl Shell {
    /// Create a new Shell socket.
    ///
    /// * `socket` - The underlying ZeroMQ Shell socket
    /// * `iopub_tx` - A channel that delivers messages to the IOPub socket
    /// * `comm_notif_socket` - Socket to receive notifications when comm events arrive
    /// * `comm_event_rx` - A channel that receives comm registration events from backend comms
    /// * `shell_handler` - The language's shell channel handler
    /// * `server_handlers` - A map of server handler target names to their handlers
    pub fn new(
        socket: Socket,
        iopub_tx: Sender<IOPubMessage>,
        comm_notif_socket: Socket,
        comm_event_rx: Receiver<CommEvent>,
        shell_handler: Box<dyn ShellHandler>,
        server_handlers: HashMap<String, Arc<Mutex<dyn ServerHandler>>>,
    ) -> Self {
        Self {
            socket,
            iopub_tx,
            shell_handler,
            server_handlers,
            comm_notif_socket,
            comm_event_rx,
            open_comms: Vec::new(),
        }
    }

    /// Main loop for the Shell thread; to be invoked by the kernel.
    pub fn listen(&mut self) {
        loop {
            log::trace!("Waiting for shell messages or comm events");

            // Poll both sockets, blocking until one is ready. We create poll_items
            // inside the loop and capture readability as bools before calling
            // &mut self methods, to avoid holding borrows across method calls.
            let (shell_readable, comm_readable) = {
                let mut poll_items = vec![
                    self.socket.socket.as_poll_item(zmq::POLLIN),
                    self.comm_notif_socket.socket.as_poll_item(zmq::POLLIN),
                ];

                // -1 means block indefinitely
                match zmq::poll(&mut poll_items, -1) {
                    Ok(0) => continue,
                    Ok(_) => (poll_items[0].is_readable(), poll_items[1].is_readable()),
                    Err(err) => {
                        log::warn!("Could not poll shell sockets: {err:?}");
                        continue;
                    },
                }
            };

            if comm_readable {
                self.process_comm_notification();
            }

            if shell_readable {
                let message = match Message::read_from_socket(&self.socket) {
                    Ok(m) => m,
                    Err(err) => {
                        log::warn!("Could not read message from shell socket: {err:?}");
                        continue;
                    },
                };

                // Handle the message; any failures while handling the messages are
                // delivered to the client instead of reported up the stack, so the
                // only errors likely here are "can't deliver to client"
                if let Err(err) = self.process_message(message) {
                    log::error!("Could not handle shell message: {err:?}");
                }
            }
        }
    }

    /// Process comm event notifications from the notifier thread.
    /// Drains all pending notifications and all pending events.
    fn process_comm_notification(&mut self) {
        // Consume all pending notifications (edge-triggered wakeups may coalesce)
        loop {
            let mut msg = zmq::Message::new();
            match self.comm_notif_socket.socket.recv(&mut msg, zmq::DONTWAIT) {
                Ok(_) => continue,
                Err(zmq::Error::EAGAIN) => break, // No more pending notifications
                Err(err) => {
                    log::error!("Could not receive comm notification: {err}");
                    break;
                },
            }
        }

        // Drain all pending comm events
        while let Ok(event) = self.comm_event_rx.try_recv() {
            self.process_comm_event(event);
        }
    }

    /// Process a comm lifecycle event from `comm_event_rx`.
    fn process_comm_event(&mut self, event: CommEvent) {
        match event {
            CommEvent::Opened(comm_socket, data) => {
                // For backend-initiated comms, notify the frontend via IOPub
                if comm_socket.initiator == CommInitiator::BackEnd {
                    self.iopub_tx
                        .send(IOPubMessage::CommOutgoing(
                            comm_socket.comm_id.clone(),
                            CommMsg::Open {
                                target_name: comm_socket.comm_name.clone(),
                                data,
                            },
                        ))
                        .log_err();
                }

                // Add the comm to our list of open comms
                self.open_comms.push(comm_socket);

                log::info!(
                    "Comm channel opened (backend); there are now {} open comms",
                    self.open_comms.len()
                );
            },

            CommEvent::Message(comm_id, msg) => {
                let Some(comm) = self.open_comms.iter().find(|c| c.comm_id == comm_id) else {
                    log::warn!("Received message for unknown comm channel {comm_id}: {msg:?}");
                    return;
                };

                log::trace!("Sending message to comm '{}'", comm.comm_name);
                comm.incoming_tx.send(msg).log_err();
            },

            CommEvent::Closed(comm_id) => {
                let Some(idx) = self.open_comms.iter().position(|c| c.comm_id == comm_id) else {
                    log::warn!("Received close message for unknown comm channel {comm_id}");
                    return;
                };

                // Notify the comm that it's being closed
                self.open_comms[idx]
                    .incoming_tx
                    .send(CommMsg::Close)
                    .log_err();

                self.open_comms.remove(idx);

                log::info!(
                    "Comm channel closed; there are now {} open comms",
                    self.open_comms.len()
                );
            },
        }
    }

    /// Process a message received from the front-end, optionally dispatching
    /// messages to the IOPub or execution threads
    fn process_message(&mut self, msg: Message) -> crate::Result<()> {
        // Extract references to the components we need to pass to handlers.
        // This allows us to borrow different fields of self independently.
        let iopub_tx = &self.iopub_tx;
        let socket = &self.socket;
        let shell_handler = &mut self.shell_handler;

        match msg {
            Message::KernelInfoRequest(req) => {
                Self::handle_request(iopub_tx, socket, req.clone(), |msg| {
                    block_on(shell_handler.handle_info_request(msg))
                        .map(kernel_info_full_reply::KernelInfoReply::from)
                })
            },
            Message::IsCompleteRequest(req) => Self::handle_request(iopub_tx, socket, req, |msg| {
                block_on(shell_handler.handle_is_complete_request(msg))
            }),
            Message::ExecuteRequest(req) => {
                // FIXME: We should ideally not pass the originator to the language kernel
                let originator = Originator::from(&req);
                Self::handle_request(iopub_tx, socket, req, |msg| {
                    block_on(shell_handler.handle_execute_request(originator, msg))
                })
            },
            Message::CompleteRequest(req) => Self::handle_request(iopub_tx, socket, req, |msg| {
                block_on(shell_handler.handle_complete_request(msg))
            }),
            Message::CommInfoRequest(req) => {
                let open_comms = &self.open_comms;
                Self::handle_request(iopub_tx, socket, req, |msg| {
                    Self::handle_comm_info_request(open_comms, msg)
                })
            },
            Message::CommOpen(req) => {
                let open_comms = &mut self.open_comms;
                let server_handlers = &self.server_handlers;
                Self::handle_notification(iopub_tx, req, |msg| {
                    Self::handle_comm_open(
                        iopub_tx,
                        shell_handler,
                        server_handlers,
                        open_comms,
                        msg,
                    )
                })
            },
            Message::CommMsg(req) => {
                let open_comms = &self.open_comms;
                let header = req.header.clone();
                Self::handle_notification(iopub_tx, req, |msg| {
                    Self::handle_comm_msg(open_comms, header, msg)
                })
            },
            Message::CommClose(req) => {
                let open_comms = &mut self.open_comms;
                Self::handle_notification(iopub_tx, req, |msg| {
                    Self::handle_comm_close(open_comms, msg)
                })
            },
            Message::InspectRequest(req) => Self::handle_request(iopub_tx, socket, req, |msg| {
                block_on(shell_handler.handle_inspect_request(msg))
            }),
            _ => Err(Error::UnsupportedMessage(msg, String::from("shell"))),
        }
    }

    /// Wrapper for all request handlers; emits busy, invokes the handler, then
    /// emits idle. Most frontends expect all shell messages to be wrapped in
    /// this pair of statuses.
    fn handle_request<Req, Rep, Handler>(
        iopub_tx: &Sender<IOPubMessage>,
        socket: &Socket,
        req: JupyterMessage<Req>,
        handler: Handler,
    ) -> crate::Result<()>
    where
        Req: ProtocolMessage,
        Rep: ProtocolMessage,
        Handler: FnOnce(&Req) -> crate::Result<Rep>,
    {
        // Enter the kernel-busy state in preparation for handling the message.
        iopub_tx
            .send(status(req.clone(), ExecutionState::Busy))
            .unwrap();

        log::info!("Received shell request: {req:?}");

        // Handle the message!
        //
        // TODO: The `handler` is currently a synchronous function, but it
        // always wraps an async function. Since the only reason we block this
        // is so we can mark the kernel as no longer busy when we're done, it'd
        // be better to take an async fn `handler` here just mark kernel as idle
        // when it finishes.
        let result = handler(&req.content);

        let result = match result {
            Ok(reply) => req.send_reply(reply, socket),
            Err(crate::Error::ShellErrorReply(error)) => req.send_error::<Rep>(error, socket),
            Err(crate::Error::ShellErrorExecuteReply(error, exec_count)) => {
                req.send_execute_error(error, exec_count, socket)
            },
            Err(err) => {
                let error = Exception::internal_error(format!("{err:?}"));
                req.send_error::<Rep>(error, socket)
            },
        };

        // Return to idle -- we always do this, even if the message generated an
        // error, since many frontends won't submit additional messages until
        // the kernel is marked idle.
        iopub_tx
            .send(status(req.clone(), ExecutionState::Idle))
            .unwrap();

        result.and(Ok(()))
    }

    fn handle_notification<Not, Handler>(
        iopub_tx: &Sender<IOPubMessage>,
        not: JupyterMessage<Not>,
        handler: Handler,
    ) -> crate::Result<()>
    where
        Not: ProtocolMessage,
        Handler: FnOnce(&Not) -> crate::Result<()>,
    {
        // Enter the kernel-busy state in preparation for handling the message
        iopub_tx
            .send(status(not.clone(), ExecutionState::Busy))
            .unwrap();

        log::info!("Received shell notification: {not:?}");

        // Handle the message
        let result = handler(&not.content);

        // Return to idle
        iopub_tx
            .send(status(not.clone(), ExecutionState::Idle))
            .unwrap();

        result
    }

    /// Handle a request for open comms
    fn handle_comm_info_request(
        open_comms: &[CommSocket],
        req: &CommInfoRequest,
    ) -> crate::Result<CommInfoReply> {
        log::info!("Received request for open comms: {req:?}");

        // Convert to a JSON object
        let mut info = serde_json::Map::new();

        for comm in open_comms.iter() {
            // Only include comms that match the target name, if one was specified
            if req.target_name.is_empty() || req.target_name == comm.comm_name {
                let comm_info_target = CommInfoTargetName {
                    target_name: comm.comm_name.clone(),
                };
                let comm_info = match serde_json::to_value(comm_info_target) {
                    Ok(v) => v,
                    Err(err) => {
                        log::error!(
                            "Failed to serialize comm info for {}: {err:?}",
                            comm.comm_name
                        );
                        continue;
                    },
                };
                info.insert(comm.comm_id.clone(), comm_info);
            }
        }

        Ok(CommInfoReply {
            status: Status::Ok,
            comms: info,
        })
    }

    /// Handle a request to open a comm
    fn handle_comm_open(
        iopub_tx: &Sender<IOPubMessage>,
        shell_handler: &mut Box<dyn ShellHandler>,
        server_handlers: &HashMap<String, Arc<Mutex<dyn ServerHandler>>>,
        open_comms: &mut Vec<CommSocket>,
        msg: &CommOpen,
    ) -> crate::Result<()> {
        log::info!("Received request to open comm: {msg:?}");

        // Process the comm open request
        let result = Self::open_comm(iopub_tx, shell_handler, server_handlers, open_comms, msg);

        // There is no error reply for a comm open request. Instead we must send
        // a `comm_close` message as soon as possible. The error is logged on our side.
        if let Err(err) = result {
            iopub_tx
                .send(IOPubMessage::CommOutgoing(
                    msg.comm_id.clone(),
                    CommMsg::Close,
                ))
                .unwrap();
            log::warn!("Failed to open comm: {err:?}");
        }

        Ok(())
    }

    /// Deliver a request from the frontend to a comm. Specifically, this is a
    /// request from the frontend to deliver a message to a backend, often as
    /// the request side of a request/response pair.
    fn handle_comm_msg(
        open_comms: &[CommSocket],
        header: JupyterHeader,
        msg: &CommWireMsg,
    ) -> crate::Result<()> {
        // The presence of an `id` field means this is a request, not a notification
        // https://github.com/posit-dev/positron/issues/7448
        let comm_msg = if msg.data.get("id").is_some() {
            // Note that the JSON-RPC `id` field must exactly match the one in
            // the Jupyter header
            let request_id = header.msg_id.clone();

            // Include the header so it can be echoed back in the reply for
            // proper message parenting
            CommMsg::Rpc {
                id: request_id,
                parent_header: header,
                data: msg.data.clone(),
            }
        } else {
            CommMsg::Data(msg.data.clone())
        };

        // Send the message to the comm
        let Some(comm) = open_comms.iter().find(|c| c.comm_id == msg.comm_id) else {
            log::warn!(
                "Received message for unknown comm channel {}: {comm_msg:?}",
                msg.comm_id
            );
            return Ok(());
        };

        log::trace!("Sending message to comm '{}'", comm.comm_name);
        comm.incoming_tx.send(comm_msg).log_err();

        Ok(())
    }

    /**
     * Performs the body of the comm open request; wrapped in a separate method to make
     * it easier to handle errors and return to the idle state when the request is
     * complete.
     */
    fn open_comm(
        iopub_tx: &Sender<IOPubMessage>,
        shell_handler: &mut Box<dyn ShellHandler>,
        server_handlers: &HashMap<String, Arc<Mutex<dyn ServerHandler>>>,
        open_comms: &mut Vec<CommSocket>,
        msg: &CommOpen,
    ) -> crate::Result<()> {
        // Check to see whether the target name begins with "positron." This
        // prefix designates comm IDs that are known to the Positron IDE.
        let comm = match msg.target_name.starts_with("positron.") {
            // This is a known comm ID; parse it by stripping the prefix and
            // matching against the known comm types
            true => match Comm::from_str(&msg.target_name[9..]) {
                Ok(comm) => comm,
                Err(err) => {
                    // If the target name starts with "positron." but we don't
                    // recognize the remainder of the string, consider that name
                    // to be invalid and return an error.
                    log::warn!(
                        "Failed to open comm; target name '{}' is unrecognized: {}",
                        &msg.target_name,
                        err
                    );
                    return Err(Error::UnknownCommName(msg.target_name.clone()));
                },
            },

            // Non-Positron comm IDs (i.e. those that don't start with
            // "positron.") are passed through to the kernel without judgment.
            // These include Jupyter comm IDs, etc.
            false => Comm::Other(msg.target_name.clone()),
        };

        // Create a comm socket for this comm. The initiator is FrontEnd here
        // because we're processing a request from the frontend to open a comm.
        let comm_id = msg.comm_id.clone();
        let comm_name = msg.target_name.clone();
        let comm_socket = CommSocket::new(
            CommInitiator::FrontEnd,
            comm_id.clone(),
            comm_name.clone(),
            iopub_tx.clone(),
        );

        // Optional notification channel used by server comms to indicate
        // they are ready to accept connections
        let mut server_started_rx: Option<Receiver<ServerStartedMessage>> = None;

        // Create a routine to send messages to the frontend over the IOPub
        // channel. This routine will be passed to the comm channel so it can
        // deliver messages to the frontend without having to store its own
        // internal ID or a reference to the IOPub channel.

        let mut lsp_comm = false;

        let opened = match comm {
            Comm::Lsp => {
                // If this is an old-style server comm (only the LSP as of now),
                // start the server and create a comm that wraps it
                lsp_comm = true;

                // Extract the target name (strip "positron." prefix if present)
                let target_key = if msg.target_name.starts_with("positron.") {
                    &msg.target_name[9..]
                } else {
                    &msg.target_name
                };

                let handler = server_handlers.get(target_key).cloned();
                server_started_rx = Some(Self::start_server_comm(msg, handler, &comm_socket)?);
                true
            },

            Comm::Other(_) => {
                // This might be a server comm or a regular comm
                if let Some(handler) = server_handlers.get(&msg.target_name).cloned() {
                    server_started_rx =
                        Some(Self::start_server_comm(msg, Some(handler), &comm_socket)?);
                    true
                } else {
                    // No server handler found, pass through to shell handler
                    block_on(shell_handler.handle_comm_open(comm, comm_socket.clone()))?
                }
            },

            // All comms tied to known Positron clients are passed through to the shell handler
            _ => {
                // Call the shell handler to open the comm
                block_on(shell_handler.handle_comm_open(comm, comm_socket.clone()))?
            },
        };

        if !opened {
            // Fail if the comm was not opened
            return Err(Error::UnknownCommName(comm_name.clone()));
        }

        // Add the comm to our list of open comms
        open_comms.push(comm_socket.clone());

        log::info!(
            "Comm channel opened; there are now {} open comms",
            open_comms.len()
        );

        // If the comm wraps a server, send notification once the server is ready to
        // accept connections. This also sends back the port number to connect on. Failing
        // to send or receive this notification is a critical failure for this comm.
        if let Some(server_started_rx) = server_started_rx {
            let result = (|| -> anyhow::Result<()> {
                let params = server_started_rx.recv()?;

                let message = if lsp_comm {
                    // If this is the LSP comm, use the legacy message structure.
                    // TODO: Switch LSP comms to new message structure once we've
                    // kicked the tyres enough with the DAP comm.
                    CommMsg::Data(serde_json::json!({
                        "msg_type": "server_started",
                        "content": params
                    }))
                } else {
                    comm_rpc_message("server_started", serde_json::to_value(params)?)
                };

                comm_socket.outgoing_tx.send(message)?;

                Ok(())
            })();

            if let Err(err) = result {
                let msg = format!("With comm '{comm_name}': {err}");
                log::error!("{msg}");
                return Err(Error::SendError(msg));
            }
        }

        Ok(())
    }

    fn start_server_comm(
        msg: &CommOpen,
        handler: Option<Arc<Mutex<dyn ServerHandler>>>,
        comm_socket: &CommSocket,
    ) -> crate::Result<Receiver<ServerStartedMessage>> {
        if let Some(handler) = handler {
            let server_start = serde_json::from_value(msg.data.clone()).map_err(|error| {
                Error::InvalidCommMessage(
                    msg.target_name.clone(),
                    serde_json::to_string(&msg.data).unwrap_or(String::from("unparseable")),
                    error.to_string(),
                )
            })?;

            let (server_started_tx, server_started_rx) =
                crossbeam::channel::bounded::<ServerStartedMessage>(1);

            // Create the new comm wrapper for the server and start it in a
            // separate thread
            let comm = ServerComm::new(handler, comm_socket.outgoing_tx.clone());
            comm.start(server_start, server_started_tx)?;

            Ok(server_started_rx)
        } else {
            // If we don't have the corresponding handler, return an error
            log::error!(
                "Client attempted to start '{}', but no handler was provided by kernel.",
                msg.target_name
            );
            Err(Error::UnknownCommName(msg.target_name.clone()))
        }
    }

    /// Handle a request to close a comm
    fn handle_comm_close(open_comms: &mut Vec<CommSocket>, msg: &CommClose) -> crate::Result<()> {
        let Some(idx) = open_comms.iter().position(|c| c.comm_id == msg.comm_id) else {
            log::warn!(
                "Received close message for unknown comm channel {}",
                msg.comm_id
            );
            return Ok(());
        };

        // Notify the comm that it's being closed
        open_comms[idx].incoming_tx.send(CommMsg::Close).log_err();

        open_comms.remove(idx);

        log::info!(
            "Comm channel closed; there are now {} open comms",
            open_comms.len()
        );

        Ok(())
    }
}

/// Create IOPub status message.
fn status(parent: JupyterMessage<impl ProtocolMessage>, state: ExecutionState) -> IOPubMessage {
    let reply = KernelStatus {
        execution_state: state,
    };
    IOPubMessage::Status(parent.header, IOPubContextChannel::Shell, reply)
}
