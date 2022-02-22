/*
 * shell.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::error::Error;
use amalthea::language::shell_handler::ShellHandler;
use amalthea::wire::comm_info_reply::CommInfoReply;
use amalthea::wire::comm_info_request::CommInfoRequest;
use amalthea::wire::complete_reply::CompleteReply;
use amalthea::wire::complete_request::CompleteRequest;
use amalthea::wire::execute_reply::ExecuteReply;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::is_complete_reply::IsComplete;
use amalthea::wire::is_complete_reply::IsCompleteReply;
use amalthea::wire::is_complete_request::IsCompleteRequest;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::kernel_info_reply::KernelInfoReply;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use amalthea::wire::language_info::LanguageInfo;
use std::sync::mpsc::Sender;

pub struct Shell {
    iopub: Sender<Message>,
}

impl Shell {
    pub fn new(iopub: Sender<Message>) -> Self {
        Self { iopub: iopub }
    }
}

impl ShellHandler for Shell {
    fn handle_info_request(&self, req: KernelInfoRequest) -> Result<KernelInfoReply, Error> {
        let info = LanguageInfo {
            name: String::from("Echo"),
            version: String::from("1.0"),
            file_extension: String::from(".ech"),
            mimetype: String::from("text/echo"),
            pygments_lexer: String::new(),
            codemirror_mode: String::new(),
            nbconvert_exporter: String::new(),
        };
        Ok(KernelInfoReply {
            status: Status::Ok,
            banner: format!("Amalthea Echo {}", env!("CARGO_PKG_VERSION")),
            debugger: false,
            protocol_version: String::from("5.0"),
            help_links: Vec::new(),
            language_info: info,
        })
    }

    fn handle_complete_request(&self, req: CompleteRequest) -> Result<CompleteReply, Error> {
        // No matches in this toy implementation.
        Ok(CompleteReply {
            matches: Vec::new(),
            status: Status::Ok,
            cursor_start: 0,
            cursor_end: 0,
            metadata: serde_json::Value::Null,
        })
    }

    /// Handle a request for open comms
    fn handle_comm_info_request(&self, req: CommInfoRequest) -> Result<CommInfoReply, Error> {
        // No comms in this toy implementation.
        Ok(CommInfoReply {
            status: Status::Ok,
            comms: serde_json::Value::Null,
        })
    }

    /// Handle a request to test code for completion.
    fn handle_is_complete_request(&self, req: IsCompleteRequest) -> Result<IsCompleteReply, Error> {
        // In this echo example, the code is always complete!
        Ok(IsCompleteReply {
            status: IsComplete::Complete,
            indent: String::from(""),
        })
    }

    /// Handles an ExecuteRequest; dispatches the request to the execution
    /// thread and forwards the response
    fn handle_execute_request(&self, req: ExecuteRequest) -> Result<ExecuteReply, Error> {
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
}
