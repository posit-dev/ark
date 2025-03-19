//
// shell.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::comm_channel::Comm;
use amalthea::comm::event::CommManagerEvent;
use amalthea::language::shell_handler::ShellHandler;
use amalthea::socket::comm::CommSocket;
use amalthea::socket::stdin::StdInRequest;
use amalthea::wire::complete_reply::CompleteReply;
use amalthea::wire::complete_request::CompleteRequest;
use amalthea::wire::execute_reply::ExecuteReply;
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
use amalthea::wire::language_info::LanguageInfoPositron;
use amalthea::wire::originator::Originator;
use async_trait::async_trait;
use bus::BusReader;
use crossbeam::channel::unbounded;
use crossbeam::channel::Sender;
use harp::environment::R_ENVS;
use harp::line_ending::convert_line_endings;
use harp::line_ending::LineEnding;
use harp::object::RObject;
use harp::ParseResult;
use log::*;
use serde_json::json;
use stdext::unwrap;

use crate::help::r_help::RHelp;
use crate::help_proxy;
use crate::interface::KernelInfo;
use crate::interface::RMain;
use crate::r_task;
use crate::request::KernelRequest;
use crate::request::RRequest;
use crate::ui::UiComm;
use crate::variables::r_variables::RVariables;

pub struct Shell {
    comm_manager_tx: Sender<CommManagerEvent>,
    r_request_tx: Sender<RRequest>,
    stdin_request_tx: Sender<StdInRequest>,
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
        r_request_tx: Sender<RRequest>,
        stdin_request_tx: Sender<StdInRequest>,
        kernel_init_rx: BusReader<KernelInfo>,
        kernel_request_tx: Sender<KernelRequest>,
    ) -> Self {
        Self {
            comm_manager_tx,
            r_request_tx,
            stdin_request_tx,
            kernel_request_tx,
            kernel_init_rx,
            kernel_info: None,
        }
    }

    fn r_handle_is_complete_request(
        &self,
        req: &IsCompleteRequest,
    ) -> amalthea::Result<IsCompleteReply> {
        match harp::parse_status(&harp::ParseInput::Text(req.code.as_str())) {
            Ok(ParseResult::Complete(_)) => Ok(IsCompleteReply {
                status: IsComplete::Complete,
                indent: String::from(""),
            }),
            Ok(ParseResult::Incomplete) => Ok(IsCompleteReply {
                status: IsComplete::Incomplete,
                indent: String::from("+"),
            }),
            Err(_) | Ok(ParseResult::SyntaxError { .. }) => Ok(IsCompleteReply {
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
    ) -> amalthea::Result<KernelInfoReply> {
        // Wait here for kernel initialization if it hasn't completed. This is
        // necessary for two reasons:
        //
        // 1. The kernel info response must include the startup banner, which is
        //    not emitted until R is done starting up.
        // 2. Jupyter frontends typically wait for the kernel info response to
        //    be sent before they signal that the kernel as ready for use, so
        //    blocking here ensures that it doesn't try to execute code before R is
        //    ready.
        if self.kernel_info.is_none() {
            trace!("Got kernel info request; waiting for R to complete initialization");
            self.kernel_info = Some(self.kernel_init_rx.recv().unwrap());
            trace!("R completed initialization, replying to kernel info request");
        } else {
            trace!("Got kernel info request; R has already started, replying to kernel info request with existing kernel information")
        }
        let kernel_info = self.kernel_info.as_ref().unwrap();

        let info = LanguageInfo {
            name: String::from("R"),
            version: kernel_info.version.clone(),
            file_extension: String::from(".R"),
            mimetype: String::from("text/r"),
            pygments_lexer: None,
            codemirror_mode: None,
            nbconvert_exporter: None,
            positron: Some(LanguageInfoPositron {
                input_prompt: kernel_info.input_prompt.clone(),
                continuation_prompt: kernel_info.continuation_prompt.clone(),
            }),
        };
        Ok(KernelInfoReply {
            status: Status::Ok,
            banner: kernel_info.banner.clone(),
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
        req: &IsCompleteRequest,
    ) -> amalthea::Result<IsCompleteReply> {
        r_task(|| self.r_handle_is_complete_request(req))
    }

    /// Handles an ExecuteRequest by sending the code to the R execution thread
    /// for processing.
    async fn handle_execute_request(
        &mut self,
        originator: Originator,
        req: &ExecuteRequest,
    ) -> amalthea::Result<ExecuteReply> {
        let (response_tx, response_rx) = unbounded::<amalthea::Result<ExecuteReply>>();
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

        result
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

    /// Handles a request to open a new comm channel
    async fn handle_comm_open(&self, target: Comm, comm: CommSocket) -> amalthea::Result<bool> {
        match target {
            Comm::Variables => handle_comm_open_variables(comm, self.comm_manager_tx.clone()),
            Comm::Ui => handle_comm_open_ui(
                comm,
                self.stdin_request_tx.clone(),
                self.kernel_request_tx.clone(),
            ),
            Comm::Help => handle_comm_open_help(comm),
            _ => Ok(false),
        }
    }
}

fn handle_comm_open_variables(
    comm: CommSocket,
    comm_manager_tx: Sender<CommManagerEvent>,
) -> amalthea::Result<bool> {
    r_task(|| {
        let global_env = RObject::view(R_ENVS.global);
        RVariables::start(global_env, comm, comm_manager_tx);
        Ok(true)
    })
}

fn handle_comm_open_ui(
    comm: CommSocket,
    stdin_request_tx: Sender<StdInRequest>,
    kernel_request_tx: Sender<KernelRequest>,
) -> amalthea::Result<bool> {
    // Create a frontend to wrap the comm channel we were just given. This starts
    // a thread that proxies messages to the frontend.
    let ui_comm_tx = UiComm::start(comm, stdin_request_tx);

    // Send the frontend event channel to the execution thread so it can emit
    // events to the frontend.
    if let Err(err) = kernel_request_tx.send(KernelRequest::EstablishUiCommChannel(ui_comm_tx)) {
        log::error!("Could not deliver UI comm channel to execution thread: {err:?}");
    };

    Ok(true)
}

fn handle_comm_open_help(comm: CommSocket) -> amalthea::Result<bool> {
    r_task(|| {
        // Ensure the R help server is started, and get its port
        let r_port = unwrap!(RHelp::r_start_or_reconnect_to_help_server(), Err(err) => {
            log::error!("Could not start R help server: {err:?}");
            return Ok(false);
        });

        // Ensure our proxy help server is started, and get its port
        let proxy_port = unwrap!(help_proxy::start(r_port), Err(err) => {
            log::error!("Could not start R help proxy server: {err:?}");
            return Ok(false);
        });

        // Start the R Help handler that routes help requests
        let help_event_tx = unwrap!(RHelp::start(comm, r_port, proxy_port), Err(err) => {
            log::error!("Could not start R Help handler: {err:?}");
            return Ok(false);
        });

        // Send the help event channel to the main R thread so it can
        // emit help events, to be delivered over the help comm.
        RMain::with_mut(|main| main.set_help_fields(help_event_tx, r_port));

        Ok(true)
    })
}
