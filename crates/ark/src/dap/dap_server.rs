//
// dap_server.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::io::{BufReader, BufWriter, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use amalthea::comm::comm_channel::CommChannelMsg;
use crossbeam::channel::{bounded, unbounded, Receiver, Sender};
use crossbeam::select;
use dap::events::*;
use dap::prelude::*;
use dap::requests::*;
use dap::responses::*;
use dap::server::ServerOutput;
use dap::types::*;
use harp::session::FrameInfo;
use serde_json::json;
use stdext::result::ResultOrLog;
use stdext::spawn;

use crate::request::{debug_request_command, DebugRequest, RRequest};

use super::dap::{Dap, DapBackendEvent};

const THREAD_ID: i64 = -1;

pub fn start_dap(
    tcp_address: String,
    state: Arc<Mutex<Dap>>,
    conn_init_tx: Sender<bool>,
    r_request_tx: Sender<RRequest>,
    comm_tx: Sender<CommChannelMsg>,
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
    comm_tx: Option<Sender<CommChannelMsg>>,
}

impl<R: Read, W: Write> DapServer<R, W> {
    pub fn new(
        reader: BufReader<R>,
        writer: BufWriter<W>,
        state: Arc<Mutex<Dap>>,
        r_request_tx: Sender<RRequest>,
        comm_tx: Sender<CommChannelMsg>,
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
            let msg = CommChannelMsg::Data(json!({ "msg_type": "restart" }));
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
        let stack = { self.state.lock().unwrap().stack.clone() };

        let stack = match stack {
            Some(s) => s.into_iter().map(into_dap_frame).collect(),
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
            let msg = CommChannelMsg::Data(json!({
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

fn into_dap_frame(frame: FrameInfo) -> StackFrame {
    let name = frame.name.clone();
    let path = frame.file.clone();
    let line = frame.line;
    let column = frame.column;

    let src = Source {
        name: None,
        path: Some(path),
        source_reference: None,
        presentation_hint: None,
        origin: None,
        sources: None,
        adapter_data: None,
        checksums: None,
    };

    StackFrame {
        id: THREAD_ID,
        name,
        source: Some(src),
        line,
        column,
        end_line: None,
        end_column: None,
        can_restart: None,
        instruction_pointer_reference: None,
        module_id: None,
        presentation_hint: None,
    }
}
