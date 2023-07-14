//
// shell.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::sync::Arc;
use std::sync::Mutex;

use amalthea::comm::comm_channel::Comm;
use amalthea::comm::event::CommEvent;
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
use amalthea::wire::input_reply::InputReply;
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
use async_trait::async_trait;
use bus::Bus;
use bus::BusReader;
use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use harp::exec::r_parse_vector;
use harp::exec::ParseResult;
use harp::object::RObject;
use harp::r_lock;
use libR_sys::*;
use log::*;
use serde_json::json;
use stdext::spawn;

use crate::environment::r_environment::REnvironment;
use crate::frontend::frontend::PositronFrontend;
use crate::interface::KernelInfo;
use crate::kernel::Kernel;
use crate::request::KernelRequest;
use crate::request::RRequest;

pub struct Shell {
    comm_manager_tx: Sender<CommEvent>,
    r_request_tx: Sender<RRequest>,
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
        comm_manager_tx: Sender<CommEvent>,
        iopub_tx: Sender<IOPubMessage>,
        r_request_tx: Sender<RRequest>,
        r_request_rx: Receiver<RRequest>,
        kernel_init_tx: Bus<KernelInfo>,
        kernel_init_rx: BusReader<KernelInfo>,
        kernel_request_tx: Sender<KernelRequest>,
        kernel_request_rx: Receiver<KernelRequest>,
        input_request_tx: Sender<ShellInputRequest>,
        conn_init_rx: Receiver<bool>,
    ) -> Self {
        // Start building the kernel object. It is shared by the shell, LSP, and main threads.
        let kernel_mutex = Arc::new(Mutex::new(Kernel::new(iopub_tx.clone())));

        let kernel_clone = kernel_mutex.clone();
        spawn!("ark-shell-thread", move || {
            listen(kernel_clone, kernel_request_rx);
        });

        let kernel_clone = kernel_mutex.clone();
        let iopub_tx_clone = iopub_tx.clone();
        spawn!("ark-r-main-thread", move || {
            // Block until 0MQ is initialised before starting R to avoid
            // thread-safety issues. See https://github.com/rstudio/positron/issues/720
            if let Err(err) = conn_init_rx.recv_timeout(std::time::Duration::from_secs(3)) {
                warn!(
                    "Failed to get init notification from main thread: {:?}",
                    err
                );
            }
            drop(conn_init_rx);

            // Start the R REPL (does not return)
            crate::interface::start_r(
                kernel_clone,
                r_request_rx,
                input_request_tx,
                iopub_tx_clone,
                kernel_init_tx,
            );
        });

        Self {
            comm_manager_tx,
            r_request_tx,
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
        r_lock! {
            self.handle_is_complete_request_impl(req)
        }
    }

    /// Handles an ExecuteRequest by sending the code to the R execution thread
    /// for processing.
    async fn handle_execute_request(
        &mut self,
        originator: Option<Originator>,
        req: &ExecuteRequest,
    ) -> Result<ExecuteReply, ExecuteReplyException> {
        let (sender, receiver) = unbounded::<ExecuteResponse>();
        if let Err(err) =
            self.r_request_tx
                .send(RRequest::ExecuteCode(req.clone(), originator, sender))
        {
            warn!(
                "Could not deliver execution request to execution thread: {}",
                err
            )
        }

        // Let the shell thread know that we've executed the code.
        trace!("Code sent to R: {}", req.code);
        let result = receiver.recv().unwrap();
        match result {
            ExecuteResponse::Reply(reply) => Ok(reply),
            ExecuteResponse::ReplyException(err) => Err(err),
        }
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
            Comm::Environment => {
                r_lock! {
                    let global_env = RObject::view(R_GlobalEnv);
                    REnvironment::start(global_env, comm.clone(), self.comm_manager_tx.clone());
                    Ok(true)
                }
            },
            Comm::FrontEnd => {
                // Create a frontend to wrap the comm channel we were just given. This starts
                // a thread that proxies messages to the frontend.
                let frontend_comm = PositronFrontend::new(comm.clone());

                // Send the frontend event channel to the execution thread so it can emit
                // events to the frontend.
                if let Err(err) = self
                    .kernel_request_tx
                    .send(KernelRequest::EstablishEventChannel(
                        frontend_comm.event_tx.clone(),
                    ))
                {
                    warn!(
                        "Could not deliver frontend event channel to execution thread: {}",
                        err
                    );
                };
                Ok(true)
            },
            _ => Ok(false),
        }
    }

    /// Handles a reply to an input_request; forwarded from the Stdin channel
    async fn handle_input_reply(
        &self,
        msg: &InputReply,
        orig: Originator,
    ) -> Result<(), Exception> {
        // Send the input reply to R in the form of an ordinary execution request.
        let req = ExecuteRequest {
            code: msg.value.clone(),
            silent: true,
            store_history: false,
            user_expressions: json!({}),
            allow_stdin: false,
            stop_on_error: false,
        };
        let (sender, receiver) = unbounded::<ExecuteResponse>();
        if let Err(err) =
            self.r_request_tx
                .send(RRequest::ExecuteCode(req.clone(), Some(orig), sender))
        {
            warn!("Could not deliver input reply to execution thread: {}", err)
        }

        // Let the shell thread know that we've executed the code.
        trace!("Input reply sent to R: {}", req.code);
        let result = receiver.recv().unwrap();
        if let ExecuteResponse::ReplyException(err) = result {
            warn!("Error in input reply: {:?}", err);
        }
        Ok(())
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
