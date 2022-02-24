/*
 * shell.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::language::shell_handler::ShellHandler;
use amalthea::wire::comm_info_reply::CommInfoReply;
use amalthea::wire::comm_info_request::CommInfoRequest;
use amalthea::wire::complete_reply::CompleteReply;
use amalthea::wire::complete_request::CompleteRequest;
use amalthea::wire::exception::Exception;
use amalthea::wire::execute_reply::ExecuteReply;
use amalthea::wire::execute_reply_exception::ExecuteReplyException;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_result::ExecuteResult;
use amalthea::wire::is_complete_reply::IsComplete;
use amalthea::wire::is_complete_reply::IsCompleteReply;
use amalthea::wire::is_complete_request::IsCompleteRequest;
use amalthea::wire::jupyter_message::JupyterMessage;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::kernel_info_reply::KernelInfoReply;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use amalthea::wire::language_info::LanguageInfo;
use serde_json::json;
use std::sync::mpsc::Sender;

pub struct Shell {
    iopub: Sender<Message>,
    execution_count: u32,
}

impl Shell {
    pub fn new(iopub: Sender<Message>) -> Self {
        Self { iopub: iopub }
    }
}

impl ShellHandler for Shell {
    fn handle_info_request(&self, req: KernelInfoRequest) -> Result<KernelInfoReply, Exception> {
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

    fn handle_complete_request(&self, req: CompleteRequest) -> Result<CompleteReply, Exception> {
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
    fn handle_comm_info_request(&self, req: CommInfoRequest) -> Result<CommInfoReply, Exception> {
        // No comms in this toy implementation.
        Ok(CommInfoReply {
            status: Status::Ok,
            comms: serde_json::Value::Null,
        })
    }

    /// Handle a request to test code for completion.
    fn handle_is_complete_request(
        &self,
        req: IsCompleteRequest,
    ) -> Result<IsCompleteReply, Exception> {
        // In this echo example, the code is always complete!
        Ok(IsCompleteReply {
            status: IsComplete::Complete,
            indent: String::from(""),
        })
    }

    /// Handles an ExecuteRequest; dispatches the request to the execution
    /// thread and forwards the response
    fn handle_execute_request(
        &self,
        req: ExecuteRequest,
    ) -> Result<ExecuteReply, ExecuteReplyException> {
        // For this toy echo language, generate a result that's just the input
        // echoed back.
        let data = json!({"text/plain": req.code });
        if let Err(err) = self
            .iopub
            .send(Message::ExecuteResult(JupyterMessage::create(
                ExecuteResult {
                    execution_count: self.execution_count,
                    data: data,
                    metadata: serde_json::Value::Null,
                },
                Some(msg.header.clone()),
                &self.session,
            )))
        {
            return Err(Error::SendError(format!("{}", err)));
        }

        // Let the shell thread know that we've successfully executed the code.
        Ok(ExecuteReply {
            status: Status::Ok,
            execution_count: self.execution_count,
            user_expressions: serde_json::Value::Null,
        })
    }

    fn generate_error(&self, msg: JupyterMessage<ExecuteRequest>) -> Result<Message, Error> {
        let exception = Exception {
            ename: String::from("Generic Error"),
            evalue: String::from("Some kind of error occurred. No idea which."),
            traceback: vec![
                String::from("Frame1"),
                String::from("Frame2"),
                String::from("Frame3"),
            ],
        };
        if let Err(err) = self
            .iopub_sender
            .send(Message::ExecuteError(JupyterMessage::create(
                ExecuteError {
                    exception: exception.clone(),
                },
                Some(msg.header.clone()),
                &self.session,
            )))
        {
            return Err(Error::SendError(format!("{}", err)));
        }

        Ok(Message::ExecuteReplyException(msg.create_reply(
            ExecuteReplyException {
                status: Status::Error,
                execution_count: self.execution_count,
                exception: exception,
            },
            &self.session,
        )))
    }

    /// Handle an execution request from the front end
    pub fn handle_execute_request(
        &mut self,
        msg: JupyterMessage<ExecuteRequest>,
    ) -> Result<(), Error> {
        // If the request is to be stored in history, it should increment the
        // execution counter.
        if msg.content.store_history {
            self.execution_count = self.execution_count + 1;
        }

        // If the code is not to be executed silently, re-broadcast the
        // execution to all frontends
        if !msg.content.silent {
            if let Err(err) = self
                .iopub_sender
                .send(Message::ExecuteInput(JupyterMessage::create(
                    ExecuteInput {
                        code: msg.content.code.clone(),
                        execution_count: self.execution_count,
                    },
                    None,
                    &self.session,
                )))
            {
                warn!(
                    "Could not broadcast execution input to all front ends: {}",
                    err
                );
            }
        }

        // Generate the appropriate reply; "err" will generate a synthetic error
        let reply = match msg.content.code.as_str() {
            "err" => self.generate_error(msg)?,
            _ => self.execute_code(msg)?,
        };

        if let Err(err) = self.sender.send(reply) {
            Err(Error::SendError(format!(
                "Could not return execution to shell: {}",
                err
            )))
        } else {
            Ok(())
        }
    }
}
