//
// dap_server.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
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
use amalthea::comm::server_comm::ServerStartMessage;
use amalthea::comm::server_comm::ServerStartedMessage;
use amalthea::socket::comm::CommOutgoingTx;
use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use dap::errors::ServerError;
use dap::events::*;
use dap::prelude::*;
use dap::requests::*;
use dap::responses::*;
use dap::server::ServerOutput;
use dap::types::*;
use stdext::result::ResultExt;
use stdext::spawn;

use super::dap::Breakpoint;
use super::dap::BreakpointState;
use super::dap::Dap;
use super::dap::DapBackendEvent;
use crate::console_debug::FrameInfo;
use crate::console_debug::FrameSource;
use crate::dap::dap::DapExceptionEvent;
use crate::dap::dap::DapStoppedEvent;
use crate::dap::dap_variables::object_variables;
use crate::dap::dap_variables::RVariable;
use crate::r_task;
use crate::request::debug_request_command;
use crate::request::DebugRequest;
use crate::request::RRequest;
use crate::url::ExtUrl;

const THREAD_ID: i64 = -1;

// TODO: Handle comm close to shut down the DAP server thread.
//
// The DAP comm is allowed to persist across TCP sessions. This supports session
// switching on the frontend. Ideally the frontend would be allowed to close the
// DAP comm in addition to the DAP TCP connection, which would shut down the DAP
// server. To achive this, the DAP server, once disconnected should wait for both
// the connection becoming ready and a channel event signalling comm close. If
// the latter fires, shut the server down.

pub fn start_dap(
    state: Arc<Mutex<Dap>>,
    server_start: ServerStartMessage,
    server_started_tx: Sender<ServerStartedMessage>,
    r_request_tx: Sender<RRequest>,
    comm_tx: CommOutgoingTx,
) {
    let ip_address = server_start.ip_address();

    // Binding to port `0` to allow the OS to allocate a port for us to bind to
    let listener = TcpListener::bind(format!("{ip_address}:0",)).unwrap();

    let address = match listener.local_addr() {
        Ok(address) => address,
        Err(error) => {
            log::error!("DAP: Failed to bind to {ip_address}:0: {error}");
            return;
        },
    };

    // Get the OS allocated port
    let port = address.port();

    log::trace!("DAP: Thread starting at address {ip_address}:{port}.");

    // Send the port back to `Shell` and eventually out to the frontend so it can connect
    server_started_tx
        .send(ServerStartedMessage::new(port))
        .log_err();

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
                let event = match event {
                    Ok(event) => event,
                    Err(err) => {
                        // Channel closed, sender dropped
                        log::info!("DAP: Event channel closed: {err:?}");
                        return;
                    },
                };

                log::trace!("DAP: Got event from backend: {:?}", event);

                let event = match event {
                    DapBackendEvent::Continued => {
                        Event::Continued(ContinuedEventBody {
                            thread_id: THREAD_ID,
                            all_threads_continued: Some(true)
                        })
                    },

                    DapBackendEvent::Stopped (DapStoppedEvent{ preserve_focus }) => {
                        Event::Stopped(StoppedEventBody {
                            reason: StoppedEventReason::Step,
                            description: None,
                            thread_id: Some(THREAD_ID),
                            preserve_focus_hint: Some(preserve_focus),
                            text: None,
                            all_threads_stopped: Some(true),
                            hit_breakpoint_ids: None,
                        })
                    },

                    DapBackendEvent::Exception(DapExceptionEvent { class, message, preserve_focus }) => {
                        let text = format!("<{class}> {message}");
                        Event::Stopped(StoppedEventBody {
                            reason: StoppedEventReason::Exception,
                            description: Some(message),
                            thread_id: Some(THREAD_ID),
                            preserve_focus_hint: Some(preserve_focus),
                            text: Some(text),
                            all_threads_stopped: Some(true),
                            hit_breakpoint_ids: None,
                        })
                    },

                    DapBackendEvent::Terminated => {
                        Event::Terminated(None)
                    },

                    DapBackendEvent::BreakpointState { id, line, verified, message } => {
                        Event::Breakpoint(BreakpointEventBody {
                            reason: BreakpointEventReason::Changed,
                            breakpoint: dap::types::Breakpoint {
                                id: Some(id),
                                line: Some(Breakpoint::to_dap_line(line)),
                                verified,
                                message,
                                ..Default::default()
                            },
                        })
                    },
                };

                let mut output = output.lock().unwrap();
                if let Err(err) = output.send_event(event) {
                    log::warn!("DAP: Failed to send event, closing: {err:?}");
                    return;
                }
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
    comm_tx: Option<CommOutgoingTx>,
}

impl<R: Read, W: Write> DapServer<R, W> {
    pub fn new(
        reader: BufReader<R>,
        writer: BufWriter<W>,
        state: Arc<Mutex<Dap>>,
        r_request_tx: Sender<RRequest>,
        comm_tx: CommOutgoingTx,
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
        let req = match self.server.poll_request() {
            Ok(Some(req)) => req,
            Ok(None) => return false,
            Err(err) => {
                log::warn!("DAP: Connection closed: {err:?}");
                return false;
            },
        };
        log::trace!("DAP: Got request: {:#?}", req);

        let cmd = req.command.clone();

        let result = match cmd {
            Command::Initialize(args) => self.handle_initialize(req, args),
            Command::Attach(args) => self.handle_attach(req, args),
            Command::Disconnect(args) => self.handle_disconnect(req, args),
            Command::Restart(args) => self.handle_restart(req, args),
            Command::Threads => self.handle_threads(req),
            Command::SetBreakpoints(args) => self.handle_set_breakpoints(req, args),
            Command::SetExceptionBreakpoints(args) => {
                self.handle_set_exception_breakpoints(req, args)
            },
            Command::StackTrace(args) => self.handle_stacktrace(req, args),
            Command::Source(args) => self.handle_source(req, args),
            Command::Scopes(args) => self.handle_scopes(req, args),
            Command::Variables(args) => self.handle_variables(req, args),
            Command::Continue(args) => {
                let resp = ResponseBody::Continue(ContinueResponse {
                    all_threads_continued: Some(true),
                });
                self.handle_step(req, args, DebugRequest::Continue, resp)
            },
            Command::Next(args) => {
                self.handle_step(req, args, DebugRequest::Next, ResponseBody::Next)
            },
            Command::StepIn(args) => {
                self.handle_step(req, args, DebugRequest::StepIn, ResponseBody::StepIn)
            },
            Command::StepOut(args) => {
                self.handle_step(req, args, DebugRequest::StepOut, ResponseBody::StepOut)
            },
            Command::Pause(args) => self.handle_pause(req, args),
            _ => {
                log::warn!("DAP: Unknown request");
                let rsp = req.error("Ark DAP: Unknown request");
                self.respond(rsp)
            },
        };

        if let Err(err) = result {
            log::warn!("DAP: Handler failed, closing connection: {err:?}");
            return false;
        }

        true
    }

    fn respond(&mut self, rsp: Response) -> Result<(), ServerError> {
        log::trace!("DAP: Responding to request: {rsp:#?}");
        self.server.respond(rsp)
    }

    fn send_event(&mut self, event: Event) -> Result<(), ServerError> {
        log::trace!("DAP: Sending event: {event:#?}");
        self.server.send_event(event)
    }

    fn handle_initialize(
        &mut self,
        req: Request,
        _args: InitializeArguments,
    ) -> Result<(), ServerError> {
        let rsp = req.success(ResponseBody::Initialize(types::Capabilities {
            supports_restart_request: Some(true),
            supports_exception_info_request: Some(false),
            exception_breakpoint_filters: Some(vec![
                types::ExceptionBreakpointsFilter {
                    filter: String::from("error"),
                    label: String::from("Errors"),
                    description: Some(String::from("Break on uncaught R errors")),
                    default: Some(false),
                    supports_condition: Some(false),
                    condition_description: None,
                },
                types::ExceptionBreakpointsFilter {
                    filter: String::from("warning"),
                    label: String::from("Warnings"),
                    description: Some(String::from("Break on R warnings")),
                    default: Some(false),
                    supports_condition: Some(false),
                    condition_description: None,
                },
            ]),
            ..Default::default()
        }));
        self.respond(rsp)?;
        self.send_event(Event::Initialized)
    }

    // Handle SetBreakpoints requests from the frontend.
    //
    // Breakpoint state survives DAP server disconnections via document hashing.
    // Disconnections happen when the user uses the disconnect command (the
    // frontend automatically reconnects) or when the console session goes to
    // the background (the LSP is also disabled, so we don't receive document
    // change notifications). When we come back online, we compare the document
    // content against our stored hash to detect if breakpoints are now stale.
    //
    // Key implementation details:
    // - We use `original_line` for lookup since the frontend doesn't know about
    //   our line adjustments and always sends back the original line numbers.
    // - When a user unchecks a breakpoint, it appears as a deletion (omitted
    //   from the request). We preserve verified breakpoints as Disabled so we
    //   can restore their state when re-enabled without requiring re-sourcing.
    fn handle_set_breakpoints(
        &mut self,
        req: Request,
        args: SetBreakpointsArguments,
    ) -> Result<(), ServerError> {
        let Some(path) = args.source.path.as_ref() else {
            // We don't currently have virtual documents managed via source references
            log::warn!("Missing a path to set breakpoints for.");
            return self.respond(req.error("Missing a path to set breakpoints for"));
        };

        // We currently only support "path" URIs as Positron never sends URIs.
        // In principle the DAP frontend can negotiate whether it sends URIs or
        // file paths via the `pathFormat` field of the `Initialize` request.
        // `ExtUrl::from_file_path` canonicalizes the path to resolve symlinks.
        let uri = match ExtUrl::from_file_path(path) {
            Ok(uri) => uri,
            Err(()) => {
                log::warn!("Can't set breakpoints for non-file path: '{path}'");
                let rsp = req.success(ResponseBody::SetBreakpoints(SetBreakpointsResponse {
                    breakpoints: vec![],
                }));
                return self.respond(rsp);
            },
        };

        // Read document content to compute hash. We currently assume UTF-8 even
        // though the frontend supports files with different encodings (but
        // UTF-8 is the default).
        let doc_content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(err) => {
                // TODO: What do we do with breakpoints in virtual documents?
                log::warn!("Failed to read file '{path}': {err:?}");
                let rsp = req.error(&format!("Failed to read file: {path}"));
                return self.respond(rsp);
            },
        };

        let args_breakpoints = args.breakpoints.unwrap_or_default();

        let mut state = self.state.lock().unwrap();
        let old_breakpoints = state.breakpoints.get(&uri).cloned();

        // Breakpoints are associated with this hash. If the document has
        // changed after a reconnection, the breakpoints are no longer valid.
        let doc_hash = blake3::hash(doc_content.as_bytes());
        let doc_changed = match &old_breakpoints {
            Some((existing_hash, _)) => existing_hash != &doc_hash,
            None => true,
        };

        let new_breakpoints = if doc_changed {
            log::trace!("DAP: Document changed for {uri}, discarding old breakpoints");

            // Replace all existing breakpoints by new, unverified ones
            args_breakpoints
                .iter()
                .map(|bp| {
                    let line = Breakpoint::from_dap_line(bp.line);
                    Breakpoint {
                        id: state.next_breakpoint_id(),
                        line,
                        original_line: line,
                        state: BreakpointState::Unverified,
                        injected: false,
                    }
                })
                .collect()
        } else {
            log::trace!("DAP: Document unchanged for {uri}, preserving breakpoint states");

            // Unwrap Safety: `doc_changed` is false, so `old_breakpoints` is Some
            let (_, old_breakpoints) = old_breakpoints.unwrap();
            // Use original_line for lookup since that's what the frontend sends back
            let mut old_by_line: HashMap<u32, Breakpoint> = old_breakpoints
                .into_iter()
                .map(|bp| (bp.original_line, bp))
                .collect();

            let mut breakpoints: Vec<Breakpoint> = Vec::new();

            for bp in &args_breakpoints {
                let line = Breakpoint::from_dap_line(bp.line);

                if let Some(old_bp) = old_by_line.remove(&line) {
                    // Breakpoint already exists at this line
                    let (new_state, injected) = match old_bp.state {
                        // This breakpoint used to be verified, was disabled, and is now back
                        // online. Restore to Verified immediately.
                        BreakpointState::Disabled => (BreakpointState::Verified, old_bp.injected),
                        // Invalid breakpoints are reset to Unverified so they can be
                        // re-validated on next source.
                        BreakpointState::Invalid(_) => (BreakpointState::Unverified, false),
                        // We preserve other states (verified or unverified)
                        other => (other, old_bp.injected),
                    };

                    breakpoints.push(Breakpoint {
                        id: old_bp.id,
                        // Preserve the actual (anchored) line from previous verification
                        line: old_bp.line,
                        original_line: line,
                        state: new_state,
                        injected,
                    });
                } else {
                    // New breakpoints always start as Unverified, until they get evaluated once
                    breakpoints.push(Breakpoint {
                        id: state.next_breakpoint_id(),
                        line,
                        original_line: line,
                        state: BreakpointState::Unverified,
                        injected: false,
                    });
                }
            }

            // Remaining verified breakpoints need to be preserved in memory
            // when deleted. That's because when user unchecks a breakpoint on
            // the frontend, the breakpoint is actually deleted (i.e. omitted)
            // by a `SetBreakpoints()` request. When the user reenables the
            // breakpoint, we have to restore the verification state.
            // Unverified/Invalid breakpoints on the other hand are simply
            // dropped since there's no verified state that needs to be
            // preserved.
            for (original_line, old_bp) in old_by_line {
                if matches!(old_bp.state, BreakpointState::Verified) {
                    breakpoints.push(Breakpoint {
                        id: old_bp.id,
                        line: old_bp.line,
                        original_line,
                        state: BreakpointState::Disabled,
                        injected: true,
                    });
                }
            }

            breakpoints
        };

        log::trace!(
            "DAP: URI {uri} now has {} breakpoints:\n{:#?}",
            new_breakpoints.len(),
            new_breakpoints
        );

        let response_breakpoints: Vec<dap::types::Breakpoint> = new_breakpoints
            .iter()
            .filter(|bp| !matches!(bp.state, BreakpointState::Disabled))
            .map(|bp| {
                let message = match &bp.state {
                    BreakpointState::Invalid(reason) => Some(reason.message().to_string()),
                    _ => None,
                };
                dap::types::Breakpoint {
                    id: Some(bp.id),
                    verified: matches!(bp.state, BreakpointState::Verified),
                    line: Some(Breakpoint::to_dap_line(bp.line)),
                    message,
                    ..Default::default()
                }
            })
            .collect();

        state.breakpoints.insert(uri, (doc_hash, new_breakpoints));

        drop(state);

        let rsp = req.success(ResponseBody::SetBreakpoints(SetBreakpointsResponse {
            breakpoints: response_breakpoints,
        }));

        self.respond(rsp)
    }

    fn handle_attach(
        &mut self,
        req: Request,
        _args: AttachRequestArguments,
    ) -> Result<(), ServerError> {
        let rsp = req.success(ResponseBody::Attach);
        self.respond(rsp)?;

        self.send_event(Event::Thread(ThreadEventBody {
            reason: ThreadEventReason::Started,
            thread_id: THREAD_ID,
        }))
    }

    fn handle_disconnect(
        &mut self,
        req: Request,
        _args: DisconnectArguments,
    ) -> Result<(), ServerError> {
        // Only send `Q` if currently in a debugging session.
        let is_debugging = { self.state.lock().unwrap().is_debugging };
        if is_debugging {
            self.send_command(DebugRequest::Quit);
        }

        let rsp = req.success(ResponseBody::Disconnect);
        self.respond(rsp)
    }

    fn handle_restart<T>(&mut self, req: Request, _args: T) -> Result<(), ServerError> {
        // If connected to Positron, forward the restart command to the
        // frontend. Otherwise ignore it.
        if let Some(tx) = &self.comm_tx {
            let msg = amalthea::comm_rpc_message!("restart");
            tx.send(msg).log_err();
        }

        let rsp = req.success(ResponseBody::Restart);
        self.respond(rsp)
    }

    // All servers must respond to `Threads` requests, possibly with
    // a dummy thread as is the case here
    fn handle_threads(&mut self, req: Request) -> Result<(), ServerError> {
        let rsp = req.success(ResponseBody::Threads(ThreadsResponse {
            threads: vec![Thread {
                id: THREAD_ID,
                name: String::from("R console"),
            }],
        }));
        self.respond(rsp)
    }

    fn handle_set_exception_breakpoints(
        &mut self,
        req: Request,
        args: SetExceptionBreakpointsArguments,
    ) -> Result<(), ServerError> {
        let mut state = self.state.lock().unwrap();
        state.breakpoints_conditions = args.filters;
        drop(state);
        let rsp = req.success(ResponseBody::SetExceptionBreakpoints(
            SetExceptionBreakpointsResponse { breakpoints: None },
        ));
        self.respond(rsp)
    }

    fn handle_stacktrace(
        &mut self,
        req: Request,
        args: StackTraceArguments,
    ) -> Result<(), ServerError> {
        let stack = {
            let state = self.state.lock().unwrap();
            let fallback_sources = &state.fallback_sources;
            match &state.stack {
                Some(stack) => stack
                    .into_iter()
                    .map(|frame| into_dap_frame(frame, fallback_sources))
                    .collect(),
                _ => vec![],
            }
        };

        // Slice the stack as requested
        let n_usize = stack.len();

        let start_frame = args.start_frame.unwrap_or(0);
        let Ok(start) = usize::try_from(start_frame) else {
            let rsp = req.error(&format!("Invalid start_frame: {start_frame}"));
            return self.respond(rsp);
        };
        let start = std::cmp::min(start, n_usize);

        let end = if let Some(levels) = args.levels {
            let Ok(levels) = usize::try_from(levels) else {
                let rsp = req.error(&format!("Invalid levels: {levels}"));
                return self.respond(rsp);
            };
            std::cmp::min(start.saturating_add(levels), n_usize)
        } else {
            n_usize
        };

        let Ok(total_frames) = i64::try_from(n_usize) else {
            let rsp = req.error(&format!("Stack frame count overflows i64: {n_usize}"));
            return self.respond(rsp);
        };
        let stack = stack[start..end].to_vec();

        let rsp = req.success(ResponseBody::StackTrace(StackTraceResponse {
            stack_frames: stack,
            total_frames: Some(total_frames),
        }));

        self.respond(rsp)
    }

    fn handle_source(&mut self, req: Request, _args: SourceArguments) -> Result<(), ServerError> {
        let message = "Unsupported `source` request: {req:?}";
        log::error!("{message}");
        let rsp = req.error(message);
        self.respond(rsp)
    }

    fn handle_scopes(&mut self, req: Request, args: ScopesArguments) -> Result<(), ServerError> {
        let state = self.state.lock().unwrap();
        let frame_id_to_variables_reference = &state.frame_id_to_variables_reference;

        // Entirely possible that the requested `frame_id` doesn't have any
        // variables (like the top most frame where the call was made). We send
        // back `0` in those cases, which is an indication of "no variables".
        let variables_reference = frame_id_to_variables_reference
            .get(&args.frame_id)
            .copied()
            .unwrap_or(0);

        // Only 1 overarching scope for now
        let scopes = vec![Scope {
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
        }];

        let rsp = req.success(ResponseBody::Scopes(ScopesResponse { scopes }));

        drop(state);
        self.respond(rsp)
    }

    fn handle_variables(
        &mut self,
        req: Request,
        args: VariablesArguments,
    ) -> Result<(), ServerError> {
        let variables_reference = args.variables_reference;
        let variables = self.collect_r_variables(variables_reference);
        let variables = self.into_variables(variables);
        let rsp = req.success(ResponseBody::Variables(VariablesResponse { variables }));
        self.respond(rsp)
    }

    fn collect_r_variables(&self, variables_reference: i64) -> Vec<RVariable> {
        // Wait until we're in the `r_task()` to lock
        // See https://github.com/posit-dev/positron/issues/5024
        let state = self.state.clone();

        let variables = r_task(move || {
            let state = state.lock().unwrap();
            let variables_reference_to_r_object = &state.variables_reference_to_r_object;

            let Some(object) = variables_reference_to_r_object.get(&variables_reference) else {
                log::error!(
                    "Failed to locate R object for `variables_reference` {variables_reference}."
                );
                return Vec::new();
            };

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

    fn handle_step<A>(
        &mut self,
        req: Request,
        _args: A,
        cmd: DebugRequest,
        resp: ResponseBody,
    ) -> Result<(), ServerError> {
        self.send_command(cmd);
        let rsp = req.success(resp);
        self.respond(rsp)
    }

    fn handle_pause(&mut self, req: Request, _args: PauseArguments) -> Result<(), ServerError> {
        self.state.lock().unwrap().is_interrupting_for_debugger = true;

        log::info!("DAP: Received request to pause R, sending interrupt");
        crate::sys::control::handle_interrupt_request();

        let rsp = req.success(ResponseBody::Pause);
        self.respond(rsp)
    }

    fn send_command(&mut self, cmd: DebugRequest) {
        if let Some(tx) = &self.comm_tx {
            // If we have a comm channel (always the case as of this
            // writing) we are connected to Positron or similar. Send
            // control events so that the IDE can execute these as if they
            // were sent by the user. This ensures prompts are updated.
            let msg = amalthea::comm_rpc_message!("execute", command = debug_request_command(cmd));

            tx.send(msg).log_err();
        } else {
            // Otherwise, send command to R's `ReadConsole()` frontend method
            self.r_request_tx.send(RRequest::DebugCommand(cmd)).unwrap();
        }
    }
}

fn into_dap_frame(frame: &FrameInfo, fallback_sources: &HashMap<String, String>) -> StackFrame {
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
    let path = match source {
        FrameSource::File(path) => Some(path),
        FrameSource::Text(source) => fallback_sources.get(&source).cloned().or_else(|| {
            log::error!("Failed to find a source reference for source text: '{source}'");
            None
        }),
    };

    let src = Source {
        name: Some(source_name),
        path,
        source_reference: None,
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
