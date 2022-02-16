/*
 * shell.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::socket::socket::Socket;
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::is_complete_reply::IsComplete;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::jupyter_message::Status;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;
use crate::wire::language_info::LanguageInfo;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;
use log::{debug, trace, warn};
use std::sync::mpsc::{Receiver, Sender};

/// Wrapper for the Shell socket; receives requests for execution, etc. from the
/// front end and handles them or dispatches them to the execution thread.
pub struct Shell {
    /// The ZeroMQ Shell socket
    socket: Socket,

    /// Sends Jupyter messages to the IOPub socket (owned by another thread)
    iopub_sender: Sender<Message>,

    /// Sends Jupyter messages to the execution thread
    request_sender: Sender<Message>,

    /// Recieves replies from the execution thread
    reply_receiver: Receiver<Message>,
}

impl Shell {
    /// Create a new Shell socket.
    ///
    /// * `socket` - The underlying ZeroMQ Shell socket
    /// * `iopub_sender` - A channel that delivers messages to the IOPub socket
    /// * `sender` - A channel that delivers messages to the execution thread
    /// * `receiver` - A channel that receives messages from the execution thread
    pub fn new(
        socket: Socket,
        iopub_sender: Sender<Message>,
        sender: Sender<Message>,
        receiver: Receiver<Message>,
    ) -> Self {
        Self {
            socket: socket,
            iopub_sender: iopub_sender,
            request_sender: sender,
            reply_receiver: receiver,
        }
    }

    /// Main loop for the Shell thread; to be invoked by the kernel.
    pub fn listen(&mut self) {
        loop {
            trace!("Waiting for shell messages");
            // Attempt to read the next message from the ZeroMQ socket
            let message = match Message::read_from_socket(&self.socket) {
                Ok(m) => m,
                Err(err) => {
                    warn!("Could not read message from shell socket: {}", err);
                    continue;
                }
            };

            // Handle the message
            if let Err(err) = self.process_message(message) {
                warn!("Could not process shell message: {}", err);
            }
        }
    }

    /// Process a message received from the front-end, optionally dispatching
    /// messages to the IOPub or execution threads
    fn process_message(&mut self, msg: Message) -> Result<(), Error> {
        let result = match msg {
            Message::KernelInfoRequest(req) => {
                self.handle_request(req, |r| self.handle_info_request(r))
            }
            Message::IsCompleteRequest(req) => {
                self.handle_request(req, |r| self.handle_is_complete_request(r))
            }
            Message::ExecuteRequest(req) => {
                self.handle_request(req, |r| self.handle_execute_request(r))
            }
            Message::CompleteRequest(req) => {
                self.handle_request(req, |r| self.handle_complete_request(r))
            }
            _ => Err(Error::UnsupportedMessage(msg, String::from("shell"))),
        };

        // TODO: if result is err we should emit a error to the client?

        result
    }

    /// Wrapper for all request handlers; emits busy, invokes the handler, then
    /// emits idle. Most frontends expect all shell messages to be wrapped in
    /// this pair of statuses.
    fn handle_request<T: ProtocolMessage, H: Fn(JupyterMessage<T>) -> Result<(), Error>>(
        &self,
        req: JupyterMessage<T>,
        handler: H,
    ) -> Result<(), Error> {
        // Enter the kernel-busy state in preparation for handling the message.
        if let Err(err) = self.send_state(req.clone(), ExecutionState::Busy) {
            warn!("Failed to change kernel status to busy: {}", err)
        }

        // Handle the message!
        let result = handler(req.clone());

        // Return to idle -- we always do this, even if the message generated an
        // error, since many front ends won't submit additional messages until
        // the kernel is marked idle.
        if let Err(err) = self.send_state(req, ExecutionState::Idle) {
            warn!("Failed to restore kernel status to idle: {}", err)
        }
        result
    }

    /// Sets the kernel state by sending a message on the IOPub channel.
    fn send_state<T: ProtocolMessage>(
        &self,
        parent: JupyterMessage<T>,
        state: ExecutionState,
    ) -> Result<(), Error> {
        let reply = parent.create_reply(
            KernelStatus {
                execution_state: state,
            },
            &self.socket.session,
        );
        if let Err(err) = self.iopub_sender.send(Message::Status(reply)) {
            return Err(Error::SendError(format!("{}", err)));
        }
        Ok(())
    }

    /// Handles an ExecuteRequest; dispatches the request to the execution
    /// thread and forwards the response
    fn handle_execute_request(&self, req: JupyterMessage<ExecuteRequest>) -> Result<(), Error> {
        debug!("Received execution request {:?}", req);

        // Send request to execution thread
        if let Err(err) = self
            .request_sender
            .send(Message::ExecuteRequest(req.clone()))
        {
            return Err(Error::SendError(format!("{}", err)));
        }

        // Wait for the execution thread to process the message; this blocks
        // until we receive a response, so this is where we'll hang out until
        // the code is done executing.
        match self.reply_receiver.recv() {
            Ok(msg) => match msg {
                Message::ExecuteReply(rep) => {
                    if let Err(err) = rep.send(&self.socket) {
                        return Err(Error::SendError(format!("{}", err)));
                    }
                }
                Message::ExecuteReplyException(rep) => {
                    if let Err(err) = rep.send(&self.socket) {
                        return Err(Error::SendError(format!("{}", err)));
                    }
                }
                _ => return Err(Error::UnsupportedMessage(msg, String::from("shell"))),
            },
            Err(err) => return Err(Error::ReceiveError(format!("{}", err))),
        };
        Ok(())
    }

    /// Handle a request to test code for completion.
    fn handle_is_complete_request(
        &self,
        req: JupyterMessage<IsCompleteRequest>,
    ) -> Result<(), Error> {
        debug!("Received request to test code for completeness: {:?}", req);
        // In this echo example, the code is always complete!
        req.send_reply(
            IsCompleteReply {
                status: IsComplete::Complete,
                indent: String::from(""),
            },
            &self.socket,
        )
    }

    /// Handle a request for kernel information.
    fn handle_info_request(&self, req: JupyterMessage<KernelInfoRequest>) -> Result<(), Error> {
        debug!("Received shell information request: {:?}", req);
        let info = LanguageInfo {
            name: String::from("Echo"),
            version: String::from("1.0"),
            file_extension: String::from(".ech"),
            mimetype: String::from("text/echo"),
            pygments_lexer: String::new(),
            codemirror_mode: String::new(),
            nbconvert_exporter: String::new(),
        };
        req.send_reply(
            KernelInfoReply {
                status: Status::Ok,
                banner: format!("Amalthea {}", env!("CARGO_PKG_VERSION")),
                debugger: false,
                protocol_version: String::from("5.0"),
                help_links: Vec::new(),
                language_info: info,
            },
            &self.socket,
        )
    }

    /// Handle a request for code completion.
    fn handle_complete_request(&self, req: JupyterMessage<CompleteRequest>) -> Result<(), Error> {
        debug!("Received request to complete code: {:?}", req);
        // No matches in this toy implementation.
        req.send_reply(
            CompleteReply {
                matches: Vec::new(),
                status: Status::Ok,
                cursor_start: 0,
                cursor_end: 0,
                metadata: serde_json::Value::Null,
            },
            &self.socket,
        )
    }
}
