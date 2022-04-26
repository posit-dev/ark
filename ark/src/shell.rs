/*
 * shell.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::lsp;
use crate::r_kernel::RKernelInfo;
use crate::r_request::RRequest;
use amalthea::language::shell_handler::ShellHandler;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::comm_info_reply::CommInfoReply;
use amalthea::wire::comm_info_request::CommInfoRequest;
use amalthea::wire::comm_msg::CommMsg;
use amalthea::wire::comm_open::CommOpen;
use amalthea::wire::complete_reply::CompleteReply;
use amalthea::wire::complete_request::CompleteRequest;
use amalthea::wire::exception::Exception;
use amalthea::wire::execute_reply::ExecuteReply;
use amalthea::wire::execute_reply_exception::ExecuteReplyException;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::inspect_reply::InspectReply;
use amalthea::wire::inspect_request::InspectRequest;
use amalthea::wire::is_complete_reply::IsComplete;
use amalthea::wire::is_complete_reply::IsCompleteReply;
use amalthea::wire::is_complete_request::IsCompleteRequest;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::kernel_info_reply::KernelInfoReply;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use amalthea::wire::language_info::LanguageInfo;
use amalthea::wire::shutdown_reply::ShutdownReply;
use amalthea::wire::shutdown_request::ShutdownRequest;
use async_trait::async_trait;
use log::{debug, trace, warn};
use serde_json::json;
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;

pub struct Shell {
    req_sender: SyncSender<RRequest>,
    execution_count: u32,
    init_receiver: Arc<Mutex<Receiver<RKernelInfo>>>,
    kernel_info: Option<RKernelInfo>,
}

impl Shell {
    pub fn new(iopub: SyncSender<IOPubMessage>) -> Self {
        let iopub_sender = iopub.clone();
        let (req_sender, req_receiver) = sync_channel::<RRequest>(1);
        let (init_sender, init_receiver) = channel::<RKernelInfo>();
        thread::spawn(move || Self::execution_thread(iopub_sender, req_receiver, init_sender));
        Self {
            execution_count: 0,
            req_sender: req_sender,
            init_receiver: Arc::new(Mutex::new(init_receiver)),
            kernel_info: None,
        }
    }

    pub fn execution_thread(
        sender: SyncSender<IOPubMessage>,
        receiver: Receiver<RRequest>,
        initializer: Sender<RKernelInfo>,
    ) {
        // Start kernel (does not return)
        crate::r_interface::start_r(sender, receiver, initializer);
    }

    fn start_lsp(msg: lsp::comm::StartLsp) {
        thread::spawn(move || lsp::backend::start_lsp(msg.client_address));
    }
}

#[async_trait]
impl ShellHandler for Shell {
    async fn handle_info_request(
        &mut self,
        _req: &KernelInfoRequest,
    ) -> Result<KernelInfoReply, Exception> {
        // Wait for kernel initialization if it hasn't completed
        if self.kernel_info.is_none() {
            trace!("Got kernel info request; waiting for R to complete initialization");
            self.kernel_info = Some(self.init_receiver.lock().unwrap().recv().unwrap());
        } else {
            trace!("R already started, using existing kernel information")
        }
        let kernel_info = self.kernel_info.as_ref().unwrap();

        let info = LanguageInfo {
            name: String::from("R"),
            version: kernel_info.version.clone(),
            file_extension: String::from(".R"),
            mimetype: String::from("text/r"),
            pygments_lexer: String::new(),
            codemirror_mode: String::new(),
            nbconvert_exporter: String::new(),
        };
        Ok(KernelInfoReply {
            status: Status::Ok,
            banner: kernel_info.banner.clone(),
            debugger: false,
            protocol_version: String::from("5.3"),
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

    /// Handle a request for open comms
    async fn handle_comm_info_request(
        &self,
        _req: &CommInfoRequest,
    ) -> Result<CommInfoReply, Exception> {
        let comms = json!({
            lsp::comm::LSP_COMM_ID: "Language Server Protocol"
        });
        Ok(CommInfoReply {
            status: Status::Ok,
            comms: comms,
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
        req: &ExecuteRequest,
    ) -> Result<ExecuteReply, ExecuteReplyException> {
        if let Err(err) = self.req_sender.send(RRequest::ExecuteCode(req.clone())) {
            warn!(
                "Could not deliver execution request to execution thread: {}",
                err
            )
        }

        // Let the shell thread know that we've successfully executed the code.
        trace!("execution finished: {}", req.code);
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
            metadata: json!({}),
        })
    }

    /// Handles a request to open a new comm channel
    async fn handle_comm_open(&self, req: &CommOpen) -> Result<(), Exception> {
        if req.comm_id.eq(lsp::comm::LSP_COMM_ID) {
            // TODO: If LSP is already started, don't start another one
            let data = serde_json::from_value::<lsp::comm::StartLsp>(req.data.clone());
            match data {
                Ok(msg) => {
                    debug!(
                        "Received request to start LSP and connect to client at {}",
                        msg.client_address
                    );
                    Shell::start_lsp(msg);
                }
                Err(err) => {
                    warn!("Unexpected data for LSP comm: {:?} ({})", req.data, err);
                }
            }
        } else {
            warn!("Request to open unknown comm: {:?}", req.data);
        }
        Ok(())
    }

    async fn handle_comm_msg(&self, _req: &CommMsg) -> Result<(), Exception> {
        // NYI
        Ok(())
    }

    async fn handle_shutdown_request(
        &self,
        msg: &ShutdownRequest,
    ) -> Result<ShutdownReply, Exception> {
        debug!("Received shutdown request: {:?}", msg);
        if let Err(err) = self.req_sender.send(RRequest::Shutdown(msg.restart)) {
            warn!(
                "Could not deliver shutdown request to execution thread: {}",
                err
            )
        }
        Ok(ShutdownReply {
            restart: msg.restart,
        })
    }
}
