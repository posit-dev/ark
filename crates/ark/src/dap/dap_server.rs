//
// dap_server.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::Mutex;

use amalthea::comm::comm_channel::CommMsg;
use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use dap::events::*;
use dap::prelude::*;
use dap::requests::*;
use dap::responses::*;
use dap::server::ServerOutput;
use dap::types::*;
use serde_json::json;
use stdext::result::ResultOrLog;
use stdext::spawn;

use super::dap::Dap;
use super::dap::DapBackendEvent;
use crate::dap::dap_r_main::FrameInfo;
use crate::dap::dap_r_main::FrameSource;
use crate::dap::dap_variables::object_variables;
use crate::dap::dap_variables::RVariable;
use crate::r_task;
use crate::request::debug_request_command;
use crate::request::DebugRequest;
use crate::request::RRequest;

const THREAD_ID: i64 = -1;

pub fn start_dap(
    tcp_address: String,
    state: Arc<Mutex<Dap>>,
    conn_init_tx: Sender<bool>,
    r_request_tx: Sender<RRequest>,
    comm_tx: Sender<CommMsg>,
) {
    log::trace!("DAP: Thread starting at address {}.", tcp_address);

    let listener = TcpListener::bind(tcp_address).unwrap();

    conn_init_tx
        .send(true)
        .or_log_error("DAP: Can't send init notification");

    loop {
        log::trace!("DAP: Waiting for client");

        let stream = match listener.accept() {
            Ok((stream, addr)) => {
                log::info!("DAP: Connected to client {addr:?}");

                let mut state = state.lock().unwrap();
                state.is_connected = true;

                stream
            },
            Err(e) => {
                log::error!("DAP: Can't get client: {e:?}");
                continue;
            },
        };

        let reader = BufReader::new(&stream);
        let writer = BufWriter::new(&stream);
        let mut server = DapServer::new(
            reader,
            writer,
            state.clone(),
            r_request_tx.clone(),
            comm_tx.clone(),
        );

        let (backend_events_tx, backend_events_rx) = unbounded::<DapBackendEvent>();
        let (done_tx, done_rx) = bounded::<bool>(0);
        let output_clone = server.output.clone();

        // We need a scope to let the borrow checker know that
        // `output_clone` drops before the next iteration (it gets tangled
        // to the stack variable `stream` through `server`)
        let _ = crossbeam::thread::scope(|scope| {
            spawn!(scope, "ark-dap-events", {
                move |_| listen_dap_events(output_clone, backend_events_rx, done_rx)
            });

            // Connect the backend to the events thread
            {
                let mut state = state.lock().unwrap();
                state.backend_events_tx = Some(backend_events_tx);
            }

            loop {
                // If disconnected, break and accept a new connection to create a new server
                if !server.serve() {
                    log::trace!("DAP: Disconnected from client");
                    let mut state = state.lock().unwrap();
                    state.is_connected = false;
                    break;
                }
            }

            // Terminate the events thread
            let _ = done_tx.send(true);
        });
    }
}

// Thread that listens for events sent by the backend, usually the
// `ReadConsole()` method. These are forwarded to the DAP client.
fn listen_dap_events<W: Write>(
    output: Arc<Mutex<ServerOutput<W>>>,
    backend_events_rx: Receiver<DapBackendEvent>,
    done_rx: Receiver<bool>,
) {
    loop {
        select!(
            recv(backend_events_rx) -> event => {
                log::trace!("DAP: Got event from backend: {:?}", event);

                let event = match event.unwrap() {
                    DapBackendEvent::Continued => {
                        Event::Continued(ContinuedEventBody {
                            thread_id: THREAD_ID,
                            all_threads_continued: Some(true)
                        })
                    },

                    DapBackendEvent::Stopped => {
                        Event::Stopped(StoppedEventBody {
                            reason: StoppedEventReason::Step,
                            description: None,
                            thread_id: Some(THREAD_ID),
                            preserve_focus_hint: Some(false),
                            text: None,
                            all_threads_stopped: Some(true),
                            hit_breakpoint_ids: None,
                        })
                    },

                    DapBackendEvent::Terminated => {
                        Event::Terminated(None)
                    },
                };

                let mut output = output.lock().unwrap();
                output.send_event(event).unwrap();
            },

            // Break the loop and terminate the thread
            recv(done_rx) -> _ => { return; },
        )
    }
}

pub struct DapServer<R: Read, W: Write> {
    server: Server<R, W>,
    pub output: Arc<Mutex<ServerOutput<W>>>,
    state: Arc<Mutex<Dap>>,
    r_request_tx: Sender<RRequest>,
    comm_tx: Option<Sender<CommMsg>>,
}

impl<R: Read, W: Write> DapServer<R, W> {
    pub fn new(
        reader: BufReader<R>,
        writer: BufWriter<W>,
        state: Arc<Mutex<Dap>>,
        r_request_tx: Sender<RRequest>,
        comm_tx: Sender<CommMsg>,
    ) -> Self {
        let server = Server::new(reader, writer);
        let output = server.output.clone();
        Self {
            server,
            output,
            state,
            r_request_tx,
            comm_tx: Some(comm_tx),
        }
    }

    pub fn serve(&mut self) -> bool {
        log::trace!("DAP: Polling");
        let req = match self.server.poll_request().unwrap() {
            Some(req) => req,
            None => return false,
        };
        log::trace!("DAP: Got request: {:?}", req);

        let cmd = req.command.clone();

        match cmd {
            Command::Initialize(args) => {
                self.handle_initialize(req, args);
            },
            Command::Attach(args) => {
                self.handle_attach(req, args);
            },
            Command::Disconnect(args) => {
                self.handle_disconnect(req, args);
            },
            Command::Restart(args) => {
                self.handle_restart(req, args);
            },
            Command::Threads => {
                self.handle_threads(req);
            },
            Command::SetExceptionBreakpoints(args) => {
                self.handle_set_exception_breakpoints(req, args);
            },
            Command::StackTrace(args) => {
                self.handle_stacktrace(req, args);
            },
            Command::Source(args) => {
                self.handle_source(req, args);
            },
            Command::Scopes(args) => {
                self.handle_scopes(req, args);
            },
            Command::Variables(args) => {
                self.handle_variables(req, args);
            },
            Command::Continue(args) => {
                let resp = ResponseBody::Continue(ContinueResponse {
                    all_threads_continued: Some(true),
                });
                self.handle_step(req, args, DebugRequest::Continue, resp);
            },
            Command::Next(args) => {
                self.handle_step(req, args, DebugRequest::Next, ResponseBody::Next);
            },
            Command::StepIn(args) => {
                self.handle_step(req, args, DebugRequest::StepIn, ResponseBody::StepIn);
            },
            Command::StepOut(args) => {
                self.handle_step(req, args, DebugRequest::StepOut, ResponseBody::StepOut);
            },
            _ => {
                log::warn!("DAP: Unknown request");
                let rsp = req.error("Ark DAP: Unknown request");
                self.server.respond(rsp).unwrap();
            },
        }

        true
    }

    fn handle_initialize(&mut self, req: Request, _args: InitializeArguments) {
        let rsp = req.success(ResponseBody::Initialize(types::Capabilities {
            supports_restart_request: Some(true),
            ..Default::default()
        }));
        self.server.respond(rsp).unwrap();

        self.server.send_event(Event::Initialized).unwrap();
    }

    fn handle_attach(&mut self, req: Request, _args: AttachRequestArguments) {
        let rsp = req.success(ResponseBody::Attach);
        self.server.respond(rsp).unwrap();

        self.server
            .send_event(Event::Stopped(StoppedEventBody {
                reason: StoppedEventReason::Step,
                description: Some(String::from("Execution paused")),
                thread_id: Some(THREAD_ID),
                preserve_focus_hint: Some(false),
                text: None,
                all_threads_stopped: None,
                hit_breakpoint_ids: None,
            }))
            .unwrap();
    }

    fn handle_disconnect(&mut self, req: Request, _args: DisconnectArguments) {
        // Only send `Q` if currently in a debugging session.
        let is_debugging = { self.state.lock().unwrap().is_debugging };
        if is_debugging {
            self.send_command(DebugRequest::Quit);
        }

        let rsp = req.success(ResponseBody::Disconnect);
        self.server.respond(rsp).unwrap();
    }

    fn handle_restart<T>(&mut self, req: Request, _args: T) {
        // If connected to Positron, forward the restart command to the
        // frontend. Otherwise ignore it.
        if let Some(tx) = &self.comm_tx {
            let msg = CommMsg::Data(json!({ "msg_type": "restart" }));
            tx.send(msg).unwrap();
        }

        let rsp = req.success(ResponseBody::Restart);
        self.server.respond(rsp).unwrap();
    }

    // All servers must respond to `Threads` requests, possibly with
    // a dummy thread as is the case here
    fn handle_threads(&mut self, req: Request) {
        let rsp = req.success(ResponseBody::Threads(ThreadsResponse {
            threads: vec![Thread {
                id: THREAD_ID,
                name: String::from("Main thread"),
            }],
        }));
        self.server.respond(rsp).unwrap();
    }

    fn handle_set_exception_breakpoints(
        &mut self,
        req: Request,
        _args: SetExceptionBreakpointsArguments,
    ) {
        let rsp = req.success(ResponseBody::SetExceptionBreakpoints(
            SetExceptionBreakpointsResponse {
                breakpoints: None, // TODO
            },
        ));
        self.server.respond(rsp).unwrap();
    }

    fn handle_stacktrace(&mut self, req: Request, args: StackTraceArguments) {
        let state = self.state.lock().unwrap();
        let stack = &state.stack;
        let fallback_sources = &state.fallback_sources;

        let stack = match stack {
            Some(stack) => stack
                .into_iter()
                .map(|frame| into_dap_frame(frame, fallback_sources))
                .collect(),
            _ => vec![],
        };

        // Slice the stack as requested
        let n_usize = stack.len();
        let start: usize = args.start_frame.unwrap_or(0).try_into().unwrap();
        let start = std::cmp::min(start, n_usize);

        let end = if let Some(levels) = args.levels {
            let levels: usize = levels.try_into().unwrap();
            std::cmp::min(start + levels, n_usize)
        } else {
            n_usize
        };

        let stack = stack[start..end].to_vec();
        let n = stack.len().try_into().unwrap();

        let rsp = req.success(ResponseBody::StackTrace(StackTraceResponse {
            stack_frames: stack,
            total_frames: Some(n),
        }));

        self.server.respond(rsp).unwrap();
    }

    fn handle_source(&mut self, req: Request, args: SourceArguments) {
        // We fully expect a `source` argument to exist, it is only for backwards
        // compatibility that it could be `None`
        let Some(source) = args.source else {
            let message = "Missing `Source` to extract a `source_reference` from.";
            log::error!("{message}");
            let rsp = req.error(message);
            self.server.respond(rsp).unwrap();
            return;
        };

        // We expect a `source_reference`. If the client had a `path` then it would
        // not have asked us for the source content.
        let Some(source_reference) = source.source_reference else {
            let message = "Missing `source_reference` to locate content for.";
            log::error!("{message}");
            let rsp = req.error(message);
            self.server.respond(rsp).unwrap();
            return;
        };

        // Try to find the source content for this `source_reference`
        let Some(content) = self.find_source_content(source_reference) else {
            let message =
                "Failed to locate source content for `source_reference` {source_reference}.";
            log::error!("{message}");
            let rsp = req.error(message);
            self.server.respond(rsp).unwrap();
            return;
        };

        let rsp = req.success(ResponseBody::Source(SourceResponse {
            content,
            mime_type: None,
        }));

        self.server.respond(rsp).unwrap();
    }

    fn find_source_content(&self, source_reference: i32) -> Option<String> {
        let state = self.state.lock().unwrap();
        let fallback_sources = &state.fallback_sources;

        // Match up the requested `source_reference` with one in our `fallback_sources`
        for (current_source, current_source_reference) in fallback_sources.iter() {
            if &source_reference == current_source_reference {
                return Some(current_source.clone());
            }
        }

        None
    }

    fn handle_scopes(&mut self, req: Request, args: ScopesArguments) {
        let state = self.state.lock().unwrap();
        let frame_id_to_variables_reference = &state.frame_id_to_variables_reference;

        // Entirely possible that the requested `frame_id` doesn't have any
        // variables (like the top most frame where the call was made). We send
        // back `0` in those cases, which is an indication of "no variables".
        let variables_reference = frame_id_to_variables_reference
            .get(&args.frame_id)
            .copied()
            .unwrap_or(0);

        let mut scopes = Vec::new();

        // Only 1 overarching scope for now
        scopes.push(Scope {
            name: String::from("Locals"),
            presentation_hint: Some(ScopePresentationhint::Locals),
            variables_reference,
            named_variables: None,
            indexed_variables: None,
            expensive: false,
            source: None,
            line: None,
            column: None,
            end_line: None,
            end_column: None,
        });

        let rsp = req.success(ResponseBody::Scopes(ScopesResponse { scopes }));

        self.server.respond(rsp).unwrap();
    }

    fn handle_variables(&mut self, req: Request, args: VariablesArguments) {
        let variables_reference = args.variables_reference;
        let variables = self.collect_r_variables(variables_reference);
        let variables = self.into_variables(variables);
        let rsp = req.success(ResponseBody::Variables(VariablesResponse { variables }));
        self.server.respond(rsp).unwrap();
    }

    fn collect_r_variables(&self, variables_reference: i64) -> Vec<RVariable> {
        let state = self.state.lock().unwrap();
        let variables_reference_to_r_object = &state.variables_reference_to_r_object;

        let Some(object) = variables_reference_to_r_object.get(&variables_reference) else {
            log::error!(
                "Failed to locate R object for `variables_reference` {variables_reference}."
            );
            return Vec::new();
        };

        // Should be safe to run an r-task while paused in the debugger, tasks
        // are still run while polling within the read console hook
        let variables = r_task(|| {
            let object = object.get();
            object_variables(object.sexp)
        });

        variables
    }

    fn into_variables(&self, variables: Vec<RVariable>) -> Vec<Variable> {
        let mut state = self.state.lock().unwrap();
        let mut out = Vec::with_capacity(variables.len());

        for variable in variables.into_iter() {
            let name = variable.name;
            let value = variable.value;
            let type_field = variable.type_field;
            let variables_reference_object = variable.variables_reference_object;

            // If we have a `variables_reference_object`, then this variable is
            // structured and has children. We need a new unique
            // `variables_reference` to return that will map to this object in
            // a followup `Variables` request.
            let variables_reference = match variables_reference_object {
                Some(x) => state.insert_variables_reference_object(x),
                None => 0,
            };

            let variable = Variable {
                name,
                value,
                type_field,
                presentation_hint: None,
                evaluate_name: None,
                variables_reference,
                named_variables: None,
                indexed_variables: None,
                memory_reference: None,
            };

            out.push(variable);
        }

        out
    }

    fn handle_step<A>(&mut self, req: Request, _args: A, cmd: DebugRequest, resp: ResponseBody) {
        self.send_command(cmd);
        let rsp = req.success(resp);
        self.server.respond(rsp).unwrap();
    }

    fn send_command(&mut self, cmd: DebugRequest) {
        if let Some(tx) = &self.comm_tx {
            // If we have a comm channel (always the case as of this
            // writing) we are connected to Positron or similar. Send
            // control events so that the IDE can execute these as if they
            // were sent by the user. This ensures prompts are updated.
            let msg = CommMsg::Data(json!({
                "msg_type": "execute",
                "content": {
                    "command": debug_request_command(cmd)
                }
            }));
            tx.send(msg).unwrap();
        } else {
            // Otherwise, send command to R's `ReadConsole()` frontend method
            self.r_request_tx.send(RRequest::DebugCommand(cmd)).unwrap();
        }
    }
}

fn into_dap_frame(frame: &FrameInfo, fallback_sources: &HashMap<String, i32>) -> StackFrame {
    let id = frame.id;
    let source_name = frame.source_name.clone();
    let frame_name = frame.frame_name.clone();
    let source = frame.source.clone();
    let start_line = frame.start_line;
    let start_column = frame.start_column;
    let end_line = frame.end_line;
    let end_column = frame.end_column;

    // Retrieve either `path` or `source_reference` depending on the `source` type.
    // In the `Text` case, a `source_reference` should always exist because we loaded
    // the map with all possible text values in `start_debug()`.
    let (path, source_reference) = match source {
        FrameSource::File(path) => (Some(path), None),
        FrameSource::Text(source) => {
            let source_reference = fallback_sources.get(&source).cloned().or_else(|| {
                log::error!("Failed to find a source reference for source text: '{source}'");
                None
            });
            (None, source_reference)
        },
    };

    let src = Source {
        name: Some(source_name),
        path,
        source_reference,
        presentation_hint: None,
        origin: None,
        sources: None,
        adapter_data: None,
        checksums: None,
    };

    StackFrame {
        id,
        name: frame_name,
        source: Some(src),
        line: start_line,
        column: start_column,
        end_line: Some(end_line),
        end_column: Some(end_column),
        can_restart: None,
        instruction_pointer_reference: None,
        module_id: None,
        presentation_hint: None,
    }
}
