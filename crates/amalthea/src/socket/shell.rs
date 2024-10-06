/*
 * shell.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

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
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::inspect_reply::InspectReply;
use crate::wire::inspect_request::InspectRequest;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::jupyter_message::Status;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;
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
    shell_handler: Arc<Mutex<dyn ShellHandler>>,

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
        shell_handler: Arc<Mutex<dyn ShellHandler>>,
        lsp_handler: Option<Arc<Mutex<dyn ServerHandler>>>,
        dap_handler: Option<Arc<Mutex<dyn ServerHandler>>>,
    ) -> Self {
        Self {
            socket,
            iopub_tx,
            shell_handler,
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
                log::warn!("Could not handle shell message: {err}");
            }
        }
    }

    /// Process a message received from the front-end, optionally dispatching
    /// messages to the IOPub or execution threads
    fn process_message(&mut self, msg: Message) -> Result<(), Error> {
        match msg {
            Message::KernelInfoRequest(req) => {
                self.handle_request(req, |h, r| self.handle_info_request(h, r))
            },
            Message::IsCompleteRequest(req) => {
                self.handle_request(req, |h, r| self.handle_is_complete_request(h, r))
            },
            Message::ExecuteRequest(req) => {
                self.handle_request(req, |h, r| self.handle_execute_request(h, r))
            },
            Message::CompleteRequest(req) => {
                self.handle_request(req, |h, r| self.handle_complete_request(h, r))
            },
            Message::CommInfoRequest(req) => {
                self.handle_request(req, |h, r| self.handle_comm_info_request(h, r))
            },
            Message::CommOpen(req) => self.handle_comm_open(req),
            Message::CommMsg(req) => self.handle_request(req, |h, r| self.handle_comm_msg(h, r)),
            Message::CommClose(req) => self.handle_comm_close(req),
            Message::InspectRequest(req) => {
                self.handle_request(req, |h, r| self.handle_inspect_request(h, r))
            },
            _ => Err(Error::UnsupportedMessage(msg, String::from("shell"))),
        }
    }

    /// Wrapper for all request handlers; emits busy, invokes the handler, then
    /// emits idle. Most frontends expect all shell messages to be wrapped in
    /// this pair of statuses.
    fn handle_request<
        T: ProtocolMessage,
        H: Fn(&mut dyn ShellHandler, JupyterMessage<T>) -> Result<(), Error>,
    >(
        &self,
        req: JupyterMessage<T>,
        handler: H,
    ) -> Result<(), Error> {
        use std::ops::DerefMut;

        // Enter the kernel-busy state in preparation for handling the message.
        if let Err(err) = self.send_state(req.clone(), ExecutionState::Busy) {
            log::warn!("Failed to change kernel status to busy: {err}")
        }

        // Lock the shell handler object on this thread
        let mut shell_handler = self.shell_handler.lock().unwrap();

        // Handle the message!
        //
        // TODO: The `handler` is currently a synchronous function, but it
        // always wraps an async function. Since the only reason we block this
        // is so we can mark the kernel as no longer busy when we're done, it'd
        // be better to take an async fn `handler` here just mark kernel as idle
        // when it finishes.
        let result = handler(shell_handler.deref_mut(), req.clone());

        // Return to idle -- we always do this, even if the message generated an
        // error, since many frontends won't submit additional messages until
        // the kernel is marked idle.
        if let Err(err) = self.send_state(req, ExecutionState::Idle) {
            log::warn!("Failed to restore kernel status to idle: {err}")
        }
        result
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

    /// Handles an ExecuteRequest; dispatches the request to the execution
    /// thread and forwards the response
    fn handle_execute_request(
        &self,
        handler: &mut dyn ShellHandler,
        req: JupyterMessage<ExecuteRequest>,
    ) -> Result<(), Error> {
        log::info!("Received execution request {req:?}");
        let originator = Originator::from(&req);
        match block_on(handler.handle_execute_request(Some(originator), &req.content)) {
            Ok(reply) => {
                log::info!("Got execution reply, delivering to frontend: {reply:?}");
                let r = req.send_reply(reply, &self.socket);
                r
            },
            // FIXME: Ark already created an `ExecuteReplyException` so we use
            // `.send_reply()` instead of `.send_error()`. Can we streamline this?
            Err(err) => req.send_reply(err, &self.socket),
        }
    }

    /// Handle a request to test code for completion.
    fn handle_is_complete_request(
        &self,
        handler: &dyn ShellHandler,
        req: JupyterMessage<IsCompleteRequest>,
    ) -> Result<(), Error> {
        log::info!("Received request to test code for completeness: {req:?}");
        match block_on(handler.handle_is_complete_request(&req.content)) {
            Ok(reply) => req.send_reply(reply, &self.socket),
            Err(err) => req.send_error::<IsCompleteReply>(err, &self.socket),
        }
    }

    /// Handle a request for kernel information.
    fn handle_info_request(
        &self,
        handler: &mut dyn ShellHandler,
        req: JupyterMessage<KernelInfoRequest>,
    ) -> Result<(), Error> {
        log::info!("Received shell kernel information request: {req:?}");
        match block_on(handler.handle_info_request(&req.content)) {
            Ok(reply) => req.send_reply(reply, &self.socket),
            Err(err) => req.send_error::<KernelInfoReply>(err, &self.socket),
        }
    }

    /// Handle a request for code completion.
    fn handle_complete_request(
        &self,
        handler: &dyn ShellHandler,
        req: JupyterMessage<CompleteRequest>,
    ) -> Result<(), Error> {
        log::info!("Received request to complete code: {req:?}");
        match block_on(handler.handle_complete_request(&req.content)) {
            Ok(reply) => req.send_reply(reply, &self.socket),
            Err(err) => req.send_error::<CompleteReply>(err, &self.socket),
        }
    }

    /// Handle a request for open comms
    fn handle_comm_info_request(
        &self,
        _handler: &dyn ShellHandler,
        req: JupyterMessage<CommInfoRequest>,
    ) -> Result<(), Error> {
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
            if req.content.target_name.is_empty() || req.content.target_name == comm.name {
                let comm_info_target = CommInfoTargetName {
                    target_name: comm.name,
                };
                let comm_info = serde_json::to_value(comm_info_target).unwrap();
                info.insert(comm.id, comm_info);
            }
        }

        // Form a reply and send it
        let reply = CommInfoReply {
            status: Status::Ok,
            comms: info,
        };
        req.send_reply(reply, &self.socket)
    }

    /// Handle a request to open a comm
    fn handle_comm_open(&mut self, req: JupyterMessage<CommOpen>) -> Result<(), Error> {
        log::info!("Received request to open comm: {req:?}");

        // Enter the kernel-busy state in preparation for handling the message.
        if let Err(err) = self.send_state(req.clone(), ExecutionState::Busy) {
            log::warn!("Failed to change kernel status to busy: {err}")
        }

        // Process the comm open request
        let result = self.open_comm(req.clone());

        // Return kernel to idle state
        if let Err(err) = self.send_state(req, ExecutionState::Idle) {
            log::warn!("Failed to restore kernel status to idle: {err}")
        }

        // Return the result
        result
    }

    /// Deliver a request from the frontend to a comm. Specifically, this is a
    /// request from the frontend to deliver a message to a backend, often as
    /// the request side of a request/response pair.
    fn handle_comm_msg(
        &self,
        _handler: &dyn ShellHandler,
        req: JupyterMessage<CommWireMsg>,
    ) -> Result<(), Error> {
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
    fn open_comm(&mut self, req: JupyterMessage<CommOpen>) -> Result<(), Error> {
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
                // Lock the shell handler object on this thread.
                let handler = self.shell_handler.lock().unwrap();

                // Call the shell handler to open the comm
                match block_on(handler.handle_comm_open(comm, comm_socket.clone())) {
                    Ok(result) => result,
                    Err(err) => {
                        // If the shell handler returns an error, send it back.
                        // This is a language evaluation error, so we can send
                        // it back in that form.
                        let errname = err.ename.clone();
                        req.send_error::<CommWireMsg>(err, &self.socket)?;

                        // Return an error to the caller indicating that the
                        // comm could not be opened due to the invalid open
                        // call.
                        return Err(Error::InvalidCommMessage(
                            req.content.target_name.clone(),
                            data_str,
                            errname,
                        ));
                    },
                }
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
            // If the comm was not opened, return an error to the caller
            return Err(Error::UnknownCommName(comm_name.clone()));
        }

        Ok(())
    }

    fn start_server_comm(
        req: &JupyterMessage<CommOpen>,
        data_str: String,
        handler: Option<Arc<Mutex<dyn ServerHandler>>>,
        comm_socket: &CommSocket,
    ) -> Result<Receiver<bool>, Error> {
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
    fn handle_comm_close(&mut self, req: JupyterMessage<CommClose>) -> Result<(), Error> {
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

    /// Handle a request for code inspection
    fn handle_inspect_request(
        &self,
        handler: &dyn ShellHandler,
        req: JupyterMessage<InspectRequest>,
    ) -> Result<(), Error> {
        log::info!("Received request to introspect code: {req:?}");
        match block_on(handler.handle_inspect_request(&req.content)) {
            Ok(reply) => req.send_reply(reply, &self.socket),
            Err(err) => req.send_error::<InspectReply>(err, &self.socket),
        }
    }
}
