/*
 * mod.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::thread;

use amalthea::comm::comm_channel::Comm;
use amalthea::comm::comm_channel::CommChannelMsg;
use amalthea::language::shell_handler::ShellHandler;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::complete_reply::CompleteReply;
use amalthea::wire::complete_request::CompleteRequest;
use amalthea::wire::exception::Exception;
use amalthea::wire::execute_error::ExecuteError;
use amalthea::wire::execute_input::ExecuteInput;
use amalthea::wire::execute_reply::ExecuteReply;
use amalthea::wire::execute_reply_exception::ExecuteReplyException;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_result::ExecuteResult;
use amalthea::wire::input_reply::InputReply;
use amalthea::wire::input_request::InputRequest;
use amalthea::wire::input_request::ShellInputRequest;
use amalthea::wire::inspect_reply::InspectReply;
use amalthea::wire::inspect_request::InspectRequest;
use amalthea::wire::is_complete_reply::IsComplete;
use amalthea::wire::is_complete_reply::IsCompleteReply;
use amalthea::wire::is_complete_request::IsCompleteRequest;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::kernel_info_reply::KernelInfoReply;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use amalthea::wire::language_info::LanguageInfo;
use async_trait::async_trait;
use crossbeam::channel::Sender;
use log::warn;
use serde_json::json;

pub struct Shell {
    iopub: Sender<IOPubMessage>,
    input_tx: Option<Sender<ShellInputRequest>>,
    execution_count: u32,
}

/// Stub implementation of the shell handler for test harness
impl Shell {
    pub fn new(iopub: Sender<IOPubMessage>) -> Self {
        Self {
            iopub: iopub,
            execution_count: 0,
            input_tx: None,
        }
    }

    // Simluates an input request
    fn prompt_for_input(&self, originator: &Vec<u8>) {
        if let Some(sender) = &self.input_tx {
            if let Err(err) = sender.send(ShellInputRequest {
                originator: originator.clone(),
                request: InputRequest {
                    prompt: String::from("Amalthea Echo> "),
                    password: false,
                },
            }) {
                warn!("Could not prompt for input: {}", err);
            }
        } else {
            panic!("No input handler established!");
        }
    }
}

#[async_trait]
impl ShellHandler for Shell {
    async fn handle_info_request(
        &mut self,
        _req: &KernelInfoRequest,
    ) -> Result<KernelInfoReply, Exception> {
        let info = LanguageInfo {
            name: String::from("Test"),
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

    async fn handle_complete_request(
        &self,
        _req: &CompleteRequest,
    ) -> Result<CompleteReply, Exception> {
        // No matches in this toy implementation.
        Ok(CompleteReply {
            matches: Vec::new(),
            status: Status::Ok,
            cursor_start: 0,
            cursor_end: 0,
            metadata: json!({}),
        })
    }

    /// Handle a request to test code for completion.
    async fn handle_is_complete_request(
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
    async fn handle_execute_request(
        &mut self,
        originator: &Vec<u8>,
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

        // Keyword: "err"
        //
        // Create an artificial error if the user requested one
        if req.code == "err" {
            let exception = Exception {
                ename: String::from("Generic Error"),
                evalue: String::from("Some kind of error occurred. No idea which."),
                traceback: vec![
                    String::from("Frame1"),
                    String::from("Frame2"),
                    String::from("Frame3"),
                ],
            };

            if let Err(err) = self.iopub.send(IOPubMessage::ExecuteError(ExecuteError {
                exception: exception.clone(),
            })) {
                warn!(
                    "Could not publish error from computation {} on iopub: {}",
                    self.execution_count, err
                );
            }

            return Err(ExecuteReplyException {
                status: Status::Error,
                execution_count: self.execution_count,
                exception: exception,
            });
        }

        // Keyword: "prompt"
        //
        // Create an artificial prompt for input
        if req.code == "prompt" {
            self.prompt_for_input(&originator);
        }

        // For this toy echo language, generate a result that's just the input
        // echoed back.
        let data = json!({"text/plain": req.code });
        if let Err(err) = self.iopub.send(IOPubMessage::ExecuteResult(ExecuteResult {
            execution_count: self.execution_count,
            data: data,
            metadata: json!({}),
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
    async fn handle_inspect_request(
        &self,
        req: &InspectRequest,
    ) -> Result<InspectReply, Exception> {
        let data = match req.code.as_str() {
            "err" => {
                json!({"text/plain": "This generates an error!"})
            },
            "teapot" => {
                json!({"text/plain": "This is clearly a teapot."})
            },
            _ => serde_json::Value::Null,
        };
        Ok(InspectReply {
            status: Status::Ok,
            found: data != serde_json::Value::Null,
            data: data,
            metadata: json!({}),
        })
    }

    async fn handle_comm_open(&self, _req: Comm, comm: CommSocket) -> Result<bool, Exception> {
        // Open a test comm channel; this test comm channel is used for every
        // comm open request (regardless of the target name). It just echoes back any
        // messages it receives.
        thread::spawn(move || loop {
            match comm.incoming_rx.recv().unwrap() {
                CommChannelMsg::Data(val) => {
                    // Echo back the data we received on the comm channel to the
                    // sender.
                    comm.outgoing_tx.send(CommChannelMsg::Data(val)).unwrap();
                },
                CommChannelMsg::Rpc(id, val) => {
                    // Echo back the data we received on the comm channel to the
                    // sender as the response to the RPC, using the same ID.
                    comm.outgoing_tx.send(CommChannelMsg::Rpc(id, val)).unwrap();
                },
                CommChannelMsg::Close => {
                    // Close the channel and exit the thread.
                    break;
                },
            }
        });
        Ok(true)
    }

    async fn handle_input_reply(&self, _msg: &InputReply) -> Result<(), Exception> {
        // NYI
        Ok(())
    }

    fn establish_input_handler(&mut self, handler: Sender<ShellInputRequest>) {
        self.input_tx = Some(handler);
    }
}
