//
// dap_server.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::io::{BufReader, BufWriter, Read, Write};
use std::net::TcpListener;

use dap::events::*;
use dap::prelude::*;
use dap::requests::*;
use dap::responses::*;
use dap::types::*;

pub fn start_dap(tcp_address: String) {
    log::trace!("DAP: Thread starting at address {}.", tcp_address);

    let listener = TcpListener::bind(tcp_address).unwrap();
    let stream = match listener.accept() {
        Ok((stream, addr)) => {
            log::info!("DAP: Connected to client {addr:?}");
            stream
        },
        Err(e) => todo!("DAP: Can't get client: {e:?}"),
    };

    let reader = BufReader::new(&stream);
    let writer = BufWriter::new(&stream);
    let mut server = DapServer::new(reader, writer);

    loop {
        server.serve();
    }
}

pub struct DapServer<R: Read, W: Write> {
    server: Server<R, W>,
}

impl<R: Read, W: Write> DapServer<R, W> {
    pub fn new(reader: BufReader<R>, writer: BufWriter<W>) -> Self {
        Self {
            server: Server::new(reader, writer),
        }
    }

    pub fn serve(&mut self) {
        log::trace!("DAP: Polling");
        let req = match self.server.poll_request().unwrap() {
            Some(req) => req,
            None => todo!("Frontend has disconnected"),
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
            _ => {
                log::warn!("DAP: Unknown request");
                let rsp = req.error("Ark DAP: Unknown request");
                self.server.respond(rsp).unwrap();
            },
        }
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
                thread_id: Some(-1),
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
                id: -1,
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
        let src = Source {
            name: None,
            path: None, // TODO
            source_reference: None,
            presentation_hint: None,
            origin: None,
            sources: None,
            adapter_data: None,
            checksums: None,
        };

        let frame = StackFrame {
            id: -1,
            name: String::from("<frame>::TODO"),
            source: Some(src),
            line: -1,
            column: -1,
            end_line: None,
            end_column: None,
            can_restart: None,
            instruction_pointer_reference: None,
            module_id: None,
            presentation_hint: None,
        };

        let rsp = req.success(ResponseBody::StackTrace(StackTraceResponse {
            stack_frames: vec![frame], // TODO: Full call stack
            total_frames: Some(1),
        }));

        self.server.respond(rsp).unwrap();
    }
}
