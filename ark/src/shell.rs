/*
 * shell.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::r_kernel::RKernel;
use amalthea::language::shell_handler::ShellHandler;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::comm_info_reply::CommInfoReply;
use amalthea::wire::comm_info_request::CommInfoRequest;
use amalthea::wire::complete_reply::CompleteReply;
use amalthea::wire::complete_request::CompleteRequest;
use amalthea::wire::exception::Exception;
use amalthea::wire::execute_input::ExecuteInput;
use amalthea::wire::execute_reply::ExecuteReply;
use amalthea::wire::execute_reply_exception::ExecuteReplyException;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_result::ExecuteResult;
use amalthea::wire::inspect_reply::InspectReply;
use amalthea::wire::inspect_request::InspectRequest;
use amalthea::wire::is_complete_reply::IsComplete;
use amalthea::wire::is_complete_reply::IsCompleteReply;
use amalthea::wire::is_complete_request::IsCompleteRequest;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::kernel_info_reply::KernelInfoReply;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use amalthea::wire::language_info::LanguageInfo;
use log::warn;
use serde_json::json;
use std::sync::mpsc::Sender;
use std::thread;

use std::env;

pub struct Shell {
    iopub: Sender<IOPubMessage>,
    execution_count: u32,
}

impl Shell {
    pub fn new(iopub: Sender<IOPubMessage>) -> Self {
        let iopub_sender = iopub.clone();
        thread::spawn(move || Self::execution_thread(iopub_sender));
        Self {
            iopub: iopub,
            execution_count: 0,
        }
    }

    pub fn execution_thread(sender: Sender<IOPubMessage>) {
        RKernel::start(sender);
    }
}

impl ShellHandler for Shell {
    fn handle_info_request(&self, _req: &KernelInfoRequest) -> Result<KernelInfoReply, Exception> {
        let info = LanguageInfo {
            name: String::from("R"),
            version: String::from("4.0"), // TODO: Read the R version here
            file_extension: String::from(".R"),
            mimetype: String::from("text/r"),
            pygments_lexer: String::new(),
            codemirror_mode: String::new(),
            nbconvert_exporter: String::new(),
        };
        Ok(KernelInfoReply {
            status: Status::Ok,
            banner: format!("Ark {}", env!("CARGO_PKG_VERSION")),
            debugger: false,
            protocol_version: String::from("5.0"),
            help_links: Vec::new(),
            language_info: info,
        })
    }

    fn handle_complete_request(&self, _req: &CompleteRequest) -> Result<CompleteReply, Exception> {
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
    fn handle_comm_info_request(&self, _req: &CommInfoRequest) -> Result<CommInfoReply, Exception> {
        // No comms in this toy implementation.
        Ok(CommInfoReply {
            status: Status::Ok,
            comms: serde_json::Value::Null,
        })
    }

    /// Handle a request to test code for completion.
    fn handle_is_complete_request(
        &self,
        _req: &IsCompleteRequest,
    ) -> Result<IsCompleteReply, Exception> {
        // In this echo example, the code is always complete!
        Ok(IsCompleteReply {
            status: IsComplete::Complete,
            indent: String::from(""),
        })
    }

    /// Handles an ExecuteRequest; "executes" the code by echoing it.
    fn handle_execute_request(
        &mut self,
        req: &ExecuteRequest,
    ) -> Result<ExecuteReply, ExecuteReplyException> {
        // Increment counter if we are storing this execution in history
        if req.store_history {
            self.execution_count = self.execution_count + 1;
        }

        // If the code is not to be executed silently, re-broadcast the
        // execution to all frontends
        if !req.silent {
            if let Err(err) = self.iopub.send(IOPubMessage::ExecuteInput(ExecuteInput {
                code: req.code.clone(),
                execution_count: self.execution_count,
            })) {
                warn!(
                    "Could not broadcast execution input {} to all front ends: {}",
                    self.execution_count, err
                );
            }
        }

        // For this toy echo language, generate a result that's just the input
        // echoed back.
        let data = json!({"text/plain": req.code });
        if let Err(err) = self.iopub.send(IOPubMessage::ExecuteResult(ExecuteResult {
            execution_count: self.execution_count,
            data: data,
            metadata: serde_json::Value::Null,
        })) {
            warn!(
                "Could not publish result of computation {} on iopub: {}",
                self.execution_count, err
            );
        }

        // Let the shell thread know that we've successfully executed the code.
        Ok(ExecuteReply {
            status: Status::Ok,
            execution_count: self.execution_count,
            user_expressions: serde_json::Value::Null,
        })
    }

    /// Handles an introspection request
    fn handle_inspect_request(&self, req: &InspectRequest) -> Result<InspectReply, Exception> {
        let data = match req.code.as_str() {
            "err" => {
                json!({"text/plain": "This generates an error!"})
            }
            "teapot" => {
                json!({"text/plain": "This is clearly a teapot."})
            }
            _ => serde_json::Value::Null,
        };
        Ok(InspectReply {
            status: Status::Ok,
            found: data != serde_json::Value::Null,
            data: data,
            metadata: serde_json::Value::Null,
        })
    }
}
