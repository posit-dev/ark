/*
 * shell.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::cell::RefCell;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::Mutex;

use crossbeam::channel::Receiver;
use crossbeam::channel::SendError;
use crossbeam::channel::Sender;
use futures::executor::block_on;
use serde_json::json;
use stdext::result::ResultOrLog;

use crate::comm::comm_channel::Comm;
use crate::comm::comm_channel::CommMsg;
use crate::comm::event::CommManagerEvent;
use crate::comm::event::CommManagerInfoReply;
use crate::comm::event::CommManagerRequest;
use crate::comm::server_comm::ServerComm;
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
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::jupyter_message::Status;
use crate::wire::originator::Originator;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;

/// Wrapper for the Shell socket; receives requests for execution, etc. from the
/// frontend and handles them or dispatches them to the execution thread.
pub struct Shell {
    /// The ZeroMQ Shell socket
    socket: Socket,

    /// Sends messages to the IOPub socket (owned by another thread)
    iopub_tx: Sender<IOPubMessage>,

    /// Language-provided shell handler object
    shell_handler: RefCell<Box<dyn ShellHandler>>,

    /// Language-provided LSP handler object
    lsp_handler: Option<Arc<Mutex<dyn ServerHandler>>>,

    /// Language-provided DAP handler object
    dap_handler: Option<Arc<Mutex<dyn ServerHandler>>>,

    /// Channel used to deliver comm events to the comm manager
    comm_manager_tx: Sender<CommManagerEvent>,
}

impl Shell {
    /// Create a new Shell socket.
    ///
    /// * `socket` - The underlying ZeroMQ Shell socket
    /// * `iopub_tx` - A channel that delivers messages to the IOPub socket
    /// * `comm_manager_tx` - A channel that delivers messages to the comm manager thread
    /// * `comm_changed_rx` - A channel that receives messages from the comm manager thread
    /// * `shell_handler` - The language's shell channel handler
    /// * `lsp_handler` - The language's LSP handler, if it supports LSP
    pub fn new(
        socket: Socket,
        iopub_tx: Sender<IOPubMessage>,
        comm_manager_tx: Sender<CommManagerEvent>,
        shell_handler: Box<dyn ShellHandler>,
        lsp_handler: Option<Arc<Mutex<dyn ServerHandler>>>,
        dap_handler: Option<Arc<Mutex<dyn ServerHandler>>>,
    ) -> Self {
        Self {
            socket,
            iopub_tx,
            shell_handler: RefCell::new(shell_handler),
            lsp_handler,
            dap_handler,
            comm_manager_tx,
        }
    }

    /// Main loop for the Shell thread; to be invoked by the kernel.
    pub fn listen(&mut self) {
        // Begin listening for shell messages
        loop {
            log::trace!("Waiting for shell messages");
            // Attempt to read the next message from the ZeroMQ socket
            let message = match Message::read_from_socket(&self.socket) {
                Ok(m) => m,
                Err(err) => {
                    log::warn!("Could not read message from shell socket: {err}");
                    continue;
                },
            };

            // Handle the message; any failures while handling the messages are
            // delivered to the client instead of reported up the stack, so the
            // only errors likely here are "can't deliver to client"
            if let Err(err) = self.process_message(message) {
                log::error!("Could not handle shell message: {err}");
            }
        }
    }

    /// Process a message received from the front-end, optionally dispatching
    /// messages to the IOPub or execution threads
    fn process_message(&self, msg: Message) -> crate::Result<()> {
        let shell_handler = &mut self.shell_handler.borrow_mut();
        match msg {
            Message::KernelInfoRequest(req) => self.handle_request(req.clone(), |msg| {
                block_on(shell_handler.handle_info_request(msg))
            }),
            Message::IsCompleteRequest(req) => self.handle_request(req, |msg| {
                block_on(shell_handler.handle_is_complete_request(msg))
            }),
            Message::ExecuteRequest(req) => {
                // FIXME: We should ideally not pass the originator to the language kernel
                let originator = Originator::from(&req);
                self.handle_request(req, |msg| {
                    block_on(shell_handler.handle_execute_request(originator, msg))
                })
            },
            Message::CompleteRequest(req) => self.handle_request(req, |msg| {
                block_on(shell_handler.handle_complete_request(msg))
            }),
            Message::CommInfoRequest(req) => {
                self.handle_request(req, |msg| self.handle_comm_info_request(msg))
            },
            Message::CommOpen(req) => self.handle_comm_open(shell_handler, req),
            Message::CommMsg(req) => self.handle_comm_msg(req),
            Message::CommClose(req) => self.handle_comm_close(req),
            Message::InspectRequest(req) => self.handle_request(req, |msg| {
                block_on(shell_handler.handle_inspect_request(msg))
            }),
            _ => Err(Error::UnsupportedMessage(msg, String::from("shell"))),
        }
    }

    /// Wrapper for all request handlers; emits busy, invokes the handler, then
    /// emits idle. Most frontends expect all shell messages to be wrapped in
    /// this pair of statuses.
    fn handle_request<Req, Rep, Handler>(
        &self,
        req: JupyterMessage<Req>,
        handler: Handler,
    ) -> crate::Result<()>
    where
        Req: ProtocolMessage,
        Rep: ProtocolMessage,
        Handler: FnOnce(&Req) -> crate::Result<Rep>,
    {
        // Enter the kernel-busy state in preparation for handling the message.
        if let Err(err) = self.send_state(req.clone(), ExecutionState::Busy) {
            log::warn!("Failed to change kernel status to busy: {err}")
        }

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
            Ok(reply) => req.send_reply(reply, &self.socket),
            Err(crate::Error::ShellErrorReply(error)) => req.send_error::<Rep>(error, &self.socket),
            Err(crate::Error::ShellErrorExecuteReply(error, exec_count)) => {
                req.send_execute_error(error, exec_count, &self.socket)
            },
            Err(err) => {
                let error = Exception::internal_error(format!("{err:?}"));
                req.send_error::<Rep>(error, &self.socket)
            },
        };

        // Return to idle -- we always do this, even if the message generated an
        // error, since many frontends won't submit additional messages until
        // the kernel is marked idle.
        if let Err(err) = self.send_state(req, ExecutionState::Idle) {
            log::error!("Failed to restore kernel status to idle: {err}")
        }

        result.and(Ok(()))
    }

    /// Sets the kernel state by sending a message on the IOPub channel.
    fn send_state<T: ProtocolMessage>(
        &self,
        parent: JupyterMessage<T>,
        state: ExecutionState,
    ) -> Result<(), SendError<IOPubMessage>> {
        let reply = KernelStatus {
            execution_state: state,
        };
        let message = IOPubMessage::Status(parent.header, IOPubContextChannel::Shell, reply);
        self.iopub_tx.send(message)
    }

    /// Handle a request for open comms
    fn handle_comm_info_request(&self, req: &CommInfoRequest) -> crate::Result<CommInfoReply> {
        log::info!("Received request for open comms: {req:?}");

        // One off sender/receiver pair for this request
        let (tx, rx) = crossbeam::channel::bounded(1);

        // Request the list of open comms from the comm manager
        self.comm_manager_tx
            .send(CommManagerEvent::Request(CommManagerRequest::Info(tx)))
            .unwrap();

        // Wait on the reply
        let CommManagerInfoReply { comms } = rx.recv().unwrap();

        // Convert to a JSON object
        let mut info = serde_json::Map::new();

        for comm in comms.into_iter() {
            // Only include comms that match the target name, if one was specified
            if req.target_name.is_empty() || req.target_name == comm.name {
                let comm_info_target = CommInfoTargetName {
                    target_name: comm.name,
                };
                let comm_info = serde_json::to_value(comm_info_target).unwrap();
                info.insert(comm.id, comm_info);
            }
        }

        Ok(CommInfoReply {
            status: Status::Ok,
            comms: info,
        })
    }

    /// Handle a request to open a comm
    fn handle_comm_open(
        &self,
        shell_handler: &mut Box<dyn ShellHandler>,
        req: JupyterMessage<CommOpen>,
    ) -> crate::Result<()> {
        log::info!("Received request to open comm: {req:?}");

        // Enter the kernel-busy state in preparation for handling the message.
        if let Err(err) = self.send_state(req.clone(), ExecutionState::Busy) {
            log::warn!("Failed to change kernel status to busy: {err}")
        }

        // Process the comm open request
        let result = self.open_comm(shell_handler, req.clone());

        // Return kernel to idle state
        if let Err(err) = self.send_state(req, ExecutionState::Idle) {
            log::warn!("Failed to restore kernel status to idle: {err}")
        }

        result
    }

    /// Deliver a request from the frontend to a comm. Specifically, this is a
    /// request from the frontend to deliver a message to a backend, often as
    /// the request side of a request/response pair.
    fn handle_comm_msg(&self, req: JupyterMessage<CommWireMsg>) -> crate::Result<()> {
        log::info!("Received request to send a message on a comm: {req:?}");

        // Enter the kernel-busy state in preparation for handling the message.
        if let Err(err) = self.send_state(req.clone(), ExecutionState::Busy) {
            log::warn!("Failed to change kernel status to busy: {err}")
        }

        // Store this message as a pending RPC request so that when the comm
        // responds, we can match it up
        self.comm_manager_tx
            .send(CommManagerEvent::PendingRpc(req.header.clone()))
            .unwrap();

        // Send the message to the comm
        let msg = CommMsg::Rpc(req.header.msg_id.clone(), req.content.data.clone());
        self.comm_manager_tx
            .send(CommManagerEvent::Message(req.content.comm_id.clone(), msg))
            .unwrap();

        // Return kernel to idle state
        if let Err(err) = self.send_state(req, ExecutionState::Idle) {
            log::warn!("Failed to restore kernel status to idle: {err}")
        }
        Ok(())
    }

    /**
     * Performs the body of the comm open request; wrapped in a separate method to make
     * it easier to handle errors and return to the idle state when the request is
     * complete.
     */
    fn open_comm(
        &self,
        shell_handler: &mut Box<dyn ShellHandler>,
        req: JupyterMessage<CommOpen>,
    ) -> crate::Result<()> {
        // Check to see whether the target name begins with "positron." This
        // prefix designates comm IDs that are known to the Positron IDE.
        let comm = match req.content.target_name.starts_with("positron.") {
            // This is a known comm ID; parse it by stripping the prefix and
            // matching against the known comm types
            true => match Comm::from_str(&req.content.target_name[9..]) {
                Ok(comm) => comm,
                Err(err) => {
                    // If the target name starts with "positron." but we don't
                    // recognize the remainder of the string, consider that name
                    // to be invalid and return an error.
                    log::warn!(
                        "Failed to open comm; target name '{}' is unrecognized: {}",
                        &req.content.target_name,
                        err
                    );
                    return Err(Error::UnknownCommName(req.content.target_name));
                },
            },

            // Non-Positron comm IDs (i.e. those that don't start with
            // "positron.") are passed through to the kernel without judgment.
            // These include Jupyter comm IDs, etc.
            false => Comm::Other(req.content.target_name.clone()),
        };

        // Get the data parameter as a string (for error reporting)
        let data_str = serde_json::to_string(&req.content.data).map_err(|err| {
            Error::InvalidCommMessage(
                req.content.target_name.clone(),
                "unparseable".to_string(),
                err.to_string(),
            )
        })?;

        // Create a comm socket for this comm. The initiator is FrontEnd here
        // because we're processing a request from the frontend to open a comm.
        let comm_id = req.content.comm_id.clone();
        let comm_name = req.content.target_name.clone();
        let comm_data = req.content.data.clone();
        let comm_socket =
            CommSocket::new(CommInitiator::FrontEnd, comm_id.clone(), comm_name.clone());

        // Optional notification channel used by server comms to indicate
        // they are ready to accept connections
        let mut conn_init_rx: Option<Receiver<bool>> = None;

        // Create a routine to send messages to the frontend over the IOPub
        // channel. This routine will be passed to the comm channel so it can
        // deliver messages to the frontend without having to store its own
        // internal ID or a reference to the IOPub channel.

        let opened = match comm {
            // If this is the special LSP or DAP comms, start the server and create
            // a comm that wraps it
            Comm::Dap => {
                let init_rx = Self::start_server_comm(
                    &req,
                    data_str,
                    self.dap_handler.clone(),
                    &comm_socket,
                )?;
                conn_init_rx = Some(init_rx);
                true
            },
            Comm::Lsp => {
                let init_rx = Self::start_server_comm(
                    &req,
                    data_str,
                    self.lsp_handler.clone(),
                    &comm_socket,
                )?;
                conn_init_rx = Some(init_rx);
                true
            },

            // Only the LSP and DAP comms are handled by the Amalthea
            // kernel framework itself; all other comms are passed through
            // to the shell handler.
            _ => {
                // Call the shell handler to open the comm
                block_on(shell_handler.handle_comm_open(comm, comm_socket.clone()))?
            },
        };

        if opened {
            // Send a notification to the comm message listener thread that a new
            // comm has been opened
            self.comm_manager_tx
                .send(CommManagerEvent::Opened(comm_socket.clone(), comm_data))
                .or_log_warning(&format!(
                    "Failed to send '{}' comm open notification to listener thread",
                    comm_socket.comm_name
                ));

            // If the comm wraps a server, send notification once the
            // server is ready to accept connections
            if let Some(rx) = conn_init_rx {
                rx.recv()
                    .or_log_warning("Expected notification for server comm init");

                comm_socket
                    .outgoing_tx
                    .send(CommMsg::Data(json!({
                        "msg_type": "server_started",
                        "content": {}
                    })))
                    .or_log_warning(&format!(
                        "Failed to send '{}' comm init notification to frontend comm",
                        comm_socket.comm_name
                    ));
            }
        } else {
            // Fail if the comm was not opened
            return Err(Error::UnknownCommName(comm_name.clone()));
        }

        Ok(())
    }

    fn start_server_comm(
        req: &JupyterMessage<CommOpen>,
        data_str: String,
        handler: Option<Arc<Mutex<dyn ServerHandler>>>,
        comm_socket: &CommSocket,
    ) -> crate::Result<Receiver<bool>> {
        if let Some(handler) = handler {
            let (init_tx, init_rx) = crossbeam::channel::bounded::<bool>(1);

            // Parse the message as server address
            let address = serde_json::from_value(req.content.data.clone()).map_err(|err| {
                Error::InvalidCommMessage(
                    req.content.target_name.clone(),
                    data_str,
                    err.to_string(),
                )
            })?;

            // Create the new comm wrapper for the server and start it in a
            // separate thread
            let comm = ServerComm::new(handler, comm_socket.outgoing_tx.clone());
            comm.start(address, init_tx)?;

            Ok(init_rx)
        } else {
            // If we don't have the corresponding handler, return an error
            log::error!(
                "Client attempted to start LSP or DAP, but no handler was provided by kernel."
            );
            Err(Error::UnknownCommName(req.content.target_name.clone()))
        }
    }

    /// Handle a request to close a comm
    fn handle_comm_close(&self, req: JupyterMessage<CommClose>) -> crate::Result<()> {
        // Look for the comm in our open comms
        log::info!("Received request to close comm: {req:?}");

        // Enter the kernel-busy state in preparation for handling the message.
        if let Err(err) = self.send_state(req.clone(), ExecutionState::Busy) {
            log::warn!("Failed to change kernel status to busy: {err}")
        }

        // Send a notification to the comm message listener thread notifying it that
        // the comm has been closed
        self.comm_manager_tx
            .send(CommManagerEvent::Closed(req.content.comm_id.clone()))
            .unwrap();

        // Return kernel to idle state
        if let Err(err) = self.send_state(req, ExecutionState::Idle) {
            log::warn!("Failed to restore kernel status to idle: {err}")
        }

        Ok(())
    }
}
