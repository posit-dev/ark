//
// dap_server.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::io::{BufReader, BufWriter, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use crossbeam::channel::{bounded, Receiver, Sender};
use crossbeam::select;
use dap::events::*;
use dap::prelude::*;
use dap::requests::*;
use dap::responses::*;
use dap::server::ServerOutput;
use dap::types::*;
use harp::session::FrameInfo;
use stdext::result::ResultOrLog;
use stdext::spawn;

use crate::request::{DebugRequest, RRequest};

use super::dap::{DapBackendEvent, DapState};

const THREAD_ID: i64 = -1;

pub fn start_dap(
    tcp_address: String,
    state: Arc<Mutex<DapState>>,
    conn_init_tx: Sender<bool>,
    events_rx: Receiver<DapBackendEvent>,
    r_request_tx: Sender<RRequest>,
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
                stream
            },
            Err(e) => {
                log::error!("DAP: Can't get client: {e:?}");
                continue;
            },
        };

        let reader = BufReader::new(&stream);
        let writer = BufWriter::new(&stream);
        let mut server = DapServer::new(reader, writer, state.clone(), r_request_tx.clone());

        let (done_tx, done_rx) = bounded::<bool>(0);
        let events_rx_clone = events_rx.clone();
        let output_clone = server.output.clone();

        // We need a scope to let the borrow checker know that
        // `output_clone` drops before the next iteration (it gets tangled
        // to the stack variable `stream` through `server`)
        let _ = crossbeam::thread::scope(|scope| {
            spawn!(scope, "ark-dap-events", {
                move |_| listen_dap_events(output_clone, events_rx_clone, done_rx)
            });

            loop {
                // If disconnected, break and accept a new connection to create a new server
                if !server.serve() {
                    log::trace!("DAP: Disconnected from client");
                    state.lock().unwrap().debugging = false;
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
    _output: Arc<Mutex<ServerOutput<W>>>,
    events_rx: Receiver<DapBackendEvent>,
    done_rx: Receiver<bool>,
) {
    loop {
        select!(
            recv(events_rx) -> event => {
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

                let mut output = _output.lock().unwrap();
                output.send_event(event).unwrap();
            },

            recv(done_rx) -> _ => { return; },
        )
    }
}

pub struct DapServer<R: Read, W: Write> {
    server: Server<R, W>,
    pub output: Arc<Mutex<ServerOutput<W>>>,
    state: Arc<Mutex<DapState>>,
    r_request_tx: Sender<RRequest>,
}

impl<R: Read, W: Write> DapServer<R, W> {
    pub fn new(
        reader: BufReader<R>,
        writer: BufWriter<W>,
        state: Arc<Mutex<DapState>>,
        r_request_tx: Sender<RRequest>,
    ) -> Self {
        let server = Server::new(reader, writer);
        let output = server.output.clone();
        Self {
            server,
            output,
            state,
            r_request_tx,
        }
    }

    pub fn serve(&mut self) -> bool {
        log::trace!("DAP: Polling");
        let req = match self.server.poll_request().unwrap() {
            Some(req) => req,
            None => {
                // TODO: Quit debugger if not busy
                return false;
            },
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
            Command::Threads => {
                self.handle_threads(req);
            },
            Command::SetExceptionBreakpoints(args) => {
                self.handle_set_exception_breakpoints(req, args);
            },
            Command::StackTrace(args) => {
                self.handle_stacktrace(req, args);
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

    fn handle_stacktrace(&mut self, req: Request, _args: StackTraceArguments) {
        let stack = { self.state.lock().unwrap().stack.clone() };

        let stack = match stack {
            Some(s) if s.len() > 0 => s.into_iter().map(into_dap_frame).collect(),
            _ => vec![],
        };

        let rsp = req.success(ResponseBody::StackTrace(StackTraceResponse {
            stack_frames: stack,
            total_frames: Some(1),
        }));

        self.server.respond(rsp).unwrap();
    }

    // TODO: For Positron, we should either send these commands from the
    // REPL to have visual feedback on the prompt, or have a way to update
    // the current prompt from `read_console()`
    fn handle_step<A>(&mut self, req: Request, _args: A, cmd: DebugRequest, resp: ResponseBody) {
        self.r_request_tx.send(RRequest::DebugCommand(cmd)).unwrap();
        let rsp = req.success(resp);
        self.server.respond(rsp).unwrap();
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
