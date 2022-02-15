/*
 * shell.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::socket::signed_socket::SignedSocket;
use crate::socket::socket::Socket;
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::execute_reply::ExecuteReply;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::is_complete_reply::IsComplete;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::Status;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;
use crate::wire::language_info::LanguageInfo;
use crate::wire::status::ExecutionState;
use log::{debug, trace, warn};
use std::sync::mpsc::{Receiver, Sender};

pub struct Shell {
    socket: SignedSocket,
    state_sender: Sender<ExecutionState>,
    request_sender: Sender<Message>,
    reply_receiver: Receiver<Message>,
    execution_count: u32,
}

impl Socket for Shell {
    fn name() -> String {
        String::from("Shell")
    }

    fn kind() -> zmq::SocketType {
        zmq::ROUTER
    }
}

impl Shell {
    pub fn new(
        socket: SignedSocket,
        state_sender: Sender<ExecutionState>,
        sender: Sender<Message>,
        receiver: Receiver<Message>,
    ) -> Self {
        Self {
            execution_count: 0,
            socket: socket,
            state_sender: state_sender,
            request_sender: sender,
            reply_receiver: receiver,
        }
    }

    pub fn listen(&mut self) {
        loop {
            trace!("Waiting for shell messages");
            let message = match Message::read_from_socket(&self.socket) {
                Ok(m) => m,
                Err(err) => {
                    warn!("Could not read message from shell socket: {}", err);
                    continue;
                }
            };
            if let Err(err) = self.process_message(message) {
                warn!("Could not process shell message: {}", err);
            }
        }
    }

    fn process_message(&mut self, msg: Message) -> Result<(), Error> {
        // note! we should emit the busy /idle status BEFORE we process messages!
        // then we can include the header
        if let Err(err) = self.state_sender.send(ExecutionState::Busy) {
            warn!("Failed to change kernel status to busy: {}", err)
        }

        let result = match msg {
            Message::KernelInfoRequest(req) => self.handle_info_request(req),
            Message::IsCompleteRequest(req) => self.handle_is_complete_request(req),
            Message::ExecuteRequest(req) => self.handle_execute_request(req),
            Message::CompleteRequest(req) => self.handle_complete_request(req),
            _ => Err(Error::UnsupportedMessage(Self::name())),
        };

        // TODO: if result is err we should emit a error to the client?

        if let Err(err) = self.state_sender.send(ExecutionState::Idle) {
            warn!("Failed to restore kernel status to idle: {}", err)
        }

        result
    }

    fn handle_execute_request(&mut self, req: JupyterMessage<ExecuteRequest>) -> Result<(), Error> {
        self.execution_count = self.execution_count + 1;
        debug!("Received execution request {:?}", req);
        if let Err(err) = self
            .request_sender
            .send(Message::ExecuteRequest(req.clone()))
        {
            return Err(Error::SendError(format!("{}", err)));
        }
        match self.reply_receiver.recv() {
            Ok(msg) => match msg {
                Message::ExecuteReply(rep) => {
                    if let Err(err) = rep.send(&self.socket) {
                        return Err(Error::SendError(format!("{}", err)));
                    }
                }
                _ => return Err(Error::UnsupportedMessage(Self::name())),
            },
            Err(err) => return Err(Error::ReceiveError(format!("{}", err))),
        };
        // TODO - error returns above should still send a reply
        req.send_reply(
            ExecuteReply {
                status: Status::Ok,
                execution_count: self.execution_count,
                user_expressions: serde_json::Value::Null,
            },
            &self.socket,
        )
    }

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

    fn handle_complete_request(&self, req: JupyterMessage<CompleteRequest>) -> Result<(), Error> {
        debug!("Received request to complete code: {:?}", req);
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
