//
// shell.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Arc;
use std::sync::Mutex;

use amalthea::comm::comm_channel::Comm;
use amalthea::comm::event::CommManagerEvent;
use amalthea::language::shell_handler::ShellHandler;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::wire::complete_reply::CompleteReply;
use amalthea::wire::complete_request::CompleteRequest;
use amalthea::wire::exception::Exception;
use amalthea::wire::execute_reply::ExecuteReply;
use amalthea::wire::execute_reply_exception::ExecuteReplyException;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_response::ExecuteResponse;
use amalthea::wire::inspect_reply::InspectReply;
use amalthea::wire::inspect_request::InspectRequest;
use amalthea::wire::is_complete_reply::IsComplete;
use amalthea::wire::is_complete_reply::IsCompleteReply;
use amalthea::wire::is_complete_request::IsCompleteRequest;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::kernel_info_reply::KernelInfoReply;
use amalthea::wire::kernel_info_request::KernelInfoRequest;
use amalthea::wire::language_info::LanguageInfo;
use amalthea::wire::language_info::LanguageInfoPositron;
use amalthea::wire::originator::Originator;
use async_trait::async_trait;
use bus::BusReader;
use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use harp::exec::r_parse_vector;
use harp::exec::ParseResult;
use harp::line_ending::convert_line_endings;
use harp::line_ending::LineEnding;
use harp::object::RObject;
use libR_shim::*;
use log::*;
use serde_json::json;
use stdext::spawn;

use crate::frontend::frontend::PositronFrontend;
use crate::help::r_help::RHelp;
use crate::interface::KernelInfo;
use crate::interface::RMain;
use crate::kernel::Kernel;
use crate::plots::graphics_device;
use crate::r_task;
use crate::request::KernelRequest;
use crate::request::RRequest;
use crate::variables::r_variables::RVariables;

pub struct Shell {
    comm_manager_tx: Sender<CommManagerEvent>,
    iopub_tx: Sender<IOPubMessage>,
    r_request_tx: Sender<RRequest>,
    pub kernel: Arc<Mutex<Kernel>>,
    kernel_request_tx: Sender<KernelRequest>,
    kernel_init_rx: BusReader<KernelInfo>,
    kernel_info: Option<KernelInfo>,
}

#[derive(Debug)]
pub enum REvent {
    Prompt,
}

impl Shell {
    /// Creates a new instance of the shell message handler.
    pub fn new(
        comm_manager_tx: Sender<CommManagerEvent>,
        iopub_tx: Sender<IOPubMessage>,
        r_request_tx: Sender<RRequest>,
        kernel_init_rx: BusReader<KernelInfo>,
        kernel_request_tx: Sender<KernelRequest>,
        kernel_request_rx: Receiver<KernelRequest>,
    ) -> Self {
        // Start building the kernel object. It is shared by the shell, LSP, and main threads.
        let kernel = Arc::new(Mutex::new(Kernel::new()));

        let kernel_clone = kernel.clone();
        spawn!("ark-shell-thread", move || {
            listen(kernel_clone, kernel_request_rx);
        });

        Self {
            comm_manager_tx,
            iopub_tx,
            r_request_tx,
            kernel,
            kernel_request_tx,
            kernel_init_rx,
            kernel_info: None,
        }
    }

    /// SAFETY: Requires the R runtime lock.
    unsafe fn handle_is_complete_request_impl(
        &self,
        req: &IsCompleteRequest,
    ) -> Result<IsCompleteReply, Exception> {
        match r_parse_vector(req.code.as_str()) {
            Ok(ParseResult::Complete(_)) => Ok(IsCompleteReply {
                status: IsComplete::Complete,
                indent: String::from(""),
            }),
            Ok(ParseResult::Incomplete()) => Ok(IsCompleteReply {
                status: IsComplete::Incomplete,
                indent: String::from("+"),
            }),
            Err(_) => Ok(IsCompleteReply {
                status: IsComplete::Invalid,
                indent: String::from(""),
            }),
        }
    }
}

#[async_trait]
impl ShellHandler for Shell {
    async fn handle_info_request(
        &mut self,
        _req: &KernelInfoRequest,
    ) -> Result<KernelInfoReply, Exception> {
        // Wait here for kernel initialization if it hasn't completed. This is
        // necessary for two reasons:
        //
        // 1. The kernel info response must include the startup banner, which is
        //    not emitted until R is done starting up.
        // 2. Jupyter front ends typically wait for the kernel info response to
        //    be sent before they signal that the kernel as ready for use, so
        //    blocking here ensures that it doesn't try to execute code before R is
        //    ready.
        if self.kernel_info.is_none() {
            trace!("Got kernel info request; waiting for R to complete initialization");
            self.kernel_info = Some(self.kernel_init_rx.recv().unwrap());
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
            positron: Some(LanguageInfoPositron {
                input_prompt: kernel_info.input_prompt.clone(),
                continuation_prompt: kernel_info.continuation_prompt.clone(),
            }),
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

    /// Handle a request to test code for completion.
    async fn handle_is_complete_request(
        &self,
        req: &IsCompleteRequest,
    ) -> Result<IsCompleteReply, Exception> {
        r_task(|| unsafe { self.handle_is_complete_request_impl(req) })
    }

    /// Handles an ExecuteRequest by sending the code to the R execution thread
    /// for processing.
    async fn handle_execute_request(
        &mut self,
        originator: Option<Originator>,
        req: &ExecuteRequest,
    ) -> Result<ExecuteReply, ExecuteReplyException> {
        let (response_tx, response_rx) = unbounded::<ExecuteResponse>();
        let mut req_clone = req.clone();
        req_clone.code = convert_line_endings(&req_clone.code, LineEnding::Posix);
        if let Err(err) = self.r_request_tx.send(RRequest::ExecuteCode(
            req_clone.clone(),
            originator,
            response_tx,
        )) {
            warn!(
                "Could not deliver execution request to execution thread: {}",
                err
            )
        }

        trace!("Code sent to R: {}", req_clone.code);
        let result = response_rx.recv().unwrap();

        let result = match result {
            ExecuteResponse::Reply(reply) => Ok(reply),
            ExecuteResponse::ReplyException(err) => Err(err),
        };

        let mut kernel = self.kernel.lock().unwrap();

        // Check for pending graphics updates
        // (Important that this occurs while in the "busy" state of this ExecuteRequest
        // so that the `parent` message is set correctly in any Jupyter messages)
        unsafe {
            graphics_device::on_did_execute_request(
                self.comm_manager_tx.clone(),
                self.iopub_tx.clone(),
                kernel.positron_connected(),
            )
        };

        // Check for changes to the working directory
        if let Err(err) = kernel.poll_working_directory() {
            warn!("Error polling working directory: {}", err);
        }

        result
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
            data,
            metadata: json!({}),
        })
    }

    /// Handles a request to open a new comm channel
    async fn handle_comm_open(&self, target: Comm, comm: CommSocket) -> Result<bool, Exception> {
        match target {
            Comm::Variables => r_task(|| unsafe {
                let global_env = RObject::view(R_GlobalEnv);
                RVariables::start(global_env, comm.clone(), self.comm_manager_tx.clone());
                Ok(true)
            }),
            Comm::FrontEnd => {
                // Create a frontend to wrap the comm channel we were just given. This starts
                // a thread that proxies messages to the frontend.
                let message_tx = PositronFrontend::start(comm.clone());

                // Send the frontend event channel to the execution thread so it can emit
                // events to the frontend.
                if let Err(err) = self
                    .kernel_request_tx
                    .send(KernelRequest::EstablishFrontendChannel(message_tx.clone()))
                {
                    warn!(
                        "Could not deliver frontend event channel to execution thread: {}",
                        err
                    );
                };
                Ok(true)
            },
            Comm::Help => {
                // Start the R Help handler
                let (help_request_tx, help_reply_rx) = match RHelp::start(comm.clone()) {
                    Ok(tx) => tx,
                    Err(err) => {
                        warn!("Could not start R Help handler: {}", err);
                        return Ok(false);
                    },
                };

                // Send the help request channel to the main R thread so it can
                // emit help events, to be delivered over the help comm.
                r_task(|| {
                    let main = RMain::get_mut();
                    main.help_tx = Some(help_request_tx.clone());
                    main.help_rx = Some(help_reply_rx.clone());
                });

                Ok(true)
            },
            _ => Ok(false),
        }
    }
}

// Kernel is shared with the main R thread
fn listen(kernel_mutex: Arc<Mutex<Kernel>>, kernel_request_rx: Receiver<KernelRequest>) {
    loop {
        // Wait for an execution request from the front end.
        match kernel_request_rx.recv() {
            Ok(req) => {
                let mut kernel = kernel_mutex.lock().unwrap();
                kernel.fulfill_request(&req)
            },
            Err(err) => warn!("Could not receive execution request from kernel: {}", err),
        }
    }
}
