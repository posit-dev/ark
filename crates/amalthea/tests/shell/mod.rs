/*
 * mod.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use std::thread;

use amalthea::comm::comm_channel::Comm;
use amalthea::comm::comm_channel::CommMsg;
use amalthea::language::shell_handler::ShellHandler;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::socket::stdin::StdInRequest;
use amalthea::wire::complete_reply::CompleteReply;
use amalthea::wire::complete_request::CompleteRequest;
use amalthea::wire::exception::Exception;
use amalthea::wire::execute_error::ExecuteError;
use amalthea::wire::execute_input::ExecuteInput;
use amalthea::wire::execute_reply::ExecuteReply;
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
use amalthea::wire::originator::Originator;
use amalthea::wire::stream::Stream;
use amalthea::wire::stream::StreamOutput;
use anyhow::anyhow;
use async_trait::async_trait;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use log::warn;
use serde_json::json;

pub struct Shell {
    iopub: Sender<IOPubMessage>,
    stdin_request_tx: Sender<StdInRequest>,
    stdin_reply_rx: Receiver<amalthea::Result<InputReply>>,
    execution_count: u32,
}

/// Stub implementation of the shell handler for test harness
impl Shell {
    pub fn new(
        iopub: Sender<IOPubMessage>,
        stdin_request_tx: Sender<StdInRequest>,
        stdin_reply_rx: Receiver<amalthea::Result<InputReply>>,
    ) -> Self {
        Self {
            iopub,
            stdin_request_tx,
            stdin_reply_rx,
            execution_count: 0,
        }
    }

    // Simluates an input request
    fn prompt_for_input(&self, originator: Originator) {
        if let Err(err) = self
            .stdin_request_tx
            .send(StdInRequest::Input(ShellInputRequest {
                originator,
                request: InputRequest {
                    prompt: String::from("Amalthea Echo> "),
                    password: false,
                },
            }))
        {
            warn!("Could not prompt for input: {}", err);
        }
    }
}

#[async_trait]
impl ShellHandler for Shell {
    async fn handle_info_request(
        &mut self,
        _req: &KernelInfoRequest,
    ) -> amalthea::Result<KernelInfoReply> {
        let info = LanguageInfo {
            name: String::from("Test"),
            version: String::from("1.0"),
            file_extension: String::from(".ech"),
            mimetype: String::from("text/echo"),
            pygments_lexer: None,
            codemirror_mode: None,
            nbconvert_exporter: None,
            positron: None,
        };
        Ok(KernelInfoReply {
            status: Status::Ok,
            banner: format!("Amalthea Echo {}", env!("CARGO_PKG_VERSION")),
            debugger: false,
            help_links: Vec::new(),
            language_info: info,
        })
    }

    async fn handle_complete_request(
        &self,
        _req: &CompleteRequest,
    ) -> amalthea::Result<CompleteReply> {
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
    ) -> amalthea::Result<IsCompleteReply> {
        // In this echo example, the code is always complete!
        Ok(IsCompleteReply {
            status: IsComplete::Complete,
            indent: String::from(""),
        })
    }

    /// Handles an ExecuteRequest; "executes" the code by echoing it.
    async fn handle_execute_request(
        &mut self,
        originator: Originator,
        req: &ExecuteRequest,
    ) -> amalthea::Result<ExecuteReply> {
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
                    "Could not broadcast execution input {} to all frontends: {}",
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

            return Err(amalthea::Error::ShellErrorExecuteReply(
                exception,
                self.execution_count,
            ));
        }

        // Keyword: "prompt"
        //
        // Create an artificial prompt for input
        if req.code == "prompt" {
            self.prompt_for_input(originator);

            // Block for the reply
            let reply = self.stdin_reply_rx.recv().unwrap();

            // Echo the reply
            self.iopub
                .send(IOPubMessage::Stream(StreamOutput {
                    name: Stream::Stdout,
                    text: reply.unwrap().value,
                }))
                .unwrap();
        }

        // For this toy echo language, generate a result that's just the input
        // echoed back.
        let data = json!({"text/plain": req.code });
        self.iopub
            .send(IOPubMessage::ExecuteResult(ExecuteResult {
                execution_count: self.execution_count,
                data,
                metadata: json!({}),
            }))
            .unwrap();

        // Let the shell thread know that we've successfully executed the code.
        Ok(ExecuteReply {
            status: Status::Ok,
            execution_count: self.execution_count,
            user_expressions: serde_json::Value::Null,
        })
    }

    /// Handles an introspection request
    async fn handle_inspect_request(&self, req: &InspectRequest) -> amalthea::Result<InspectReply> {
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
            data,
            metadata: json!({}),
        })
    }

    async fn handle_comm_open(&self, req: Comm, comm: CommSocket) -> amalthea::Result<bool> {
        // Used to test error replies
        match req {
            Comm::Other(name) if name == "unknown" => {
                return Err(amalthea::Error::Anyhow(anyhow!("unknown comm target")));
            },
            _ => {},
        }

        // Open a test comm channel; this test comm channel is used for every
        // comm open request (regardless of the target name). It just echoes back any
        // messages it receives.
        thread::spawn(move || loop {
            match comm.incoming_rx.recv().unwrap() {
                CommMsg::Data(val) => {
                    // Echo back the data we received on the comm channel to the
                    // sender.
                    comm.outgoing_tx.send(CommMsg::Data(val)).unwrap();
                },
                CommMsg::Rpc(id, val) => {
                    // Echo back the data we received on the comm channel to the
                    // sender as the response to the RPC, using the same ID.
                    comm.outgoing_tx.send(CommMsg::Rpc(id, val)).unwrap();
                },
                CommMsg::Close => {
                    // Close the channel and exit the thread.
                    break;
                },
            }
        });
        Ok(true)
    }
}
