//
// dap_client.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::io::BufRead;
use std::io::BufReader;
use std::io::BufWriter;
use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::time::Duration;

use anyhow::anyhow;
use dap::base_message::BaseMessage;
use dap::base_message::Sendable;
use dap::events::BreakpointEventBody;
use dap::events::Event;
use dap::events::StoppedEventBody;
use dap::requests::AttachRequestArguments;
use dap::requests::Command;
use dap::requests::ContinueArguments;
use dap::requests::DisconnectArguments;
use dap::requests::InitializeArguments;
use dap::requests::NextArguments;
use dap::requests::Request;
use dap::requests::ScopesArguments;
use dap::requests::SetBreakpointsArguments;
use dap::requests::StackTraceArguments;
use dap::requests::StepInArguments;
use dap::requests::VariablesArguments;
use dap::responses::Response;
use dap::responses::ResponseBody;
use dap::types::Breakpoint;
use dap::types::Capabilities;
use dap::types::Scope;
use dap::types::Source;
use dap::types::SourceBreakpoint;
use dap::types::StackFrame;
use dap::types::StoppedEventReason;
use dap::types::Thread;
use dap::types::Variable;

/// Default timeout for receiving DAP messages
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// A minimal DAP client for testing purposes.
///
/// Automatically disconnects from the server when dropped.
pub struct DapClient {
    reader: BufReader<TcpStream>,
    writer: BufWriter<TcpStream>,
    seq: i64,
    connected: bool,
}

impl DapClient {
    /// Connect to a DAP server at the given address and port.
    pub fn connect(addr: &str, port: u16) -> anyhow::Result<Self> {
        let stream = TcpStream::connect(format!("{addr}:{port}"))?;
        stream.set_read_timeout(Some(DEFAULT_TIMEOUT))?;
        stream.set_write_timeout(Some(DEFAULT_TIMEOUT))?;

        let reader = BufReader::new(stream.try_clone()?);
        let writer = BufWriter::new(stream);

        Ok(Self {
            reader,
            writer,
            seq: 0,
            connected: false,
        })
    }

    /// Initialize the DAP session.
    ///
    /// Sends Initialize request, asserts success, and consumes the Initialized event.
    /// Returns the server's capabilities.
    #[track_caller]
    pub fn initialize(&mut self) -> Capabilities {
        let seq = self
            .send(Command::Initialize(InitializeArguments {
                adapter_id: String::from("ark-test"),
                client_id: Some(String::from("test-client")),
                client_name: Some(String::from("Test Client")),
                // 1-based offsets as in Positron
                lines_start_at1: Some(true),
                columns_start_at1: Some(true),
                ..Default::default()
            }))
            .unwrap();

        let response = self.recv_response(seq);
        assert!(response.success, "Initialize request failed");

        let caps = match response.body {
            Some(ResponseBody::Initialize(caps)) => caps,
            other => panic!("Expected Initialize response body, got {:?}", other),
        };

        let event = self.recv_event();
        assert!(
            matches!(event, Event::Initialized),
            "Expected Initialized event, got {:?}",
            event
        );

        self.connected = true;
        caps
    }

    /// Attach to the debuggee.
    ///
    /// Sends Attach request and consumes the Thread (started) event.
    #[track_caller]
    pub fn attach(&mut self) {
        let seq = self
            .send(Command::Attach(AttachRequestArguments {
                ..Default::default()
            }))
            .unwrap();

        let response = self.recv_response(seq);
        assert!(response.success, "Attach request failed");
        assert!(
            matches!(response.body, Some(ResponseBody::Attach)),
            "Expected Attach response body, got {:?}",
            response.body
        );

        let event = self.recv_event();
        let Event::Thread(thread) = event else {
            panic!("Expected Thread event, got {:?}", event);
        };
        assert_eq!(thread.thread_id, -1, "Expected thread_id -1");
    }

    /// Send continue execution (exit browser/debugger) to server.
    #[track_caller]
    pub fn continue_execution(&mut self) {
        let seq = self
            .send(Command::Continue(ContinueArguments {
                thread_id: -1,
                single_thread: None,
            }))
            .unwrap();

        let response = self.recv_response(seq);
        assert!(response.success, "Continue request failed");
        assert!(
            matches!(response.body, Some(ResponseBody::Continue(_))),
            "Expected Continue response body, got {:?}",
            response.body
        );
    }

    /// Send next (step over) command to server.
    #[track_caller]
    pub fn step_next(&mut self) {
        let seq = self
            .send(Command::Next(NextArguments {
                thread_id: -1,
                single_thread: None,
                granularity: None,
            }))
            .unwrap();

        let response = self.recv_response(seq);
        assert!(response.success, "Next request failed");
        assert!(
            matches!(response.body, Some(ResponseBody::Next)),
            "Expected Next response body, got {:?}",
            response.body
        );
    }

    /// Send step in command to server.
    #[track_caller]
    pub fn step_in(&mut self) {
        let seq = self
            .send(Command::StepIn(StepInArguments {
                thread_id: -1,
                single_thread: None,
                target_id: None,
                granularity: None,
            }))
            .unwrap();

        let response = self.recv_response(seq);
        assert!(response.success, "StepIn request failed");
        assert!(
            matches!(response.body, Some(ResponseBody::StepIn)),
            "Expected StepIn response body, got {:?}",
            response.body
        );
    }

    /// Set breakpoints for a source file.
    ///
    /// Takes a file path and a list of line numbers (1-based).
    /// Returns the breakpoints as reported by the server.
    #[track_caller]
    pub fn set_breakpoints(&mut self, path: &str, lines: &[i64]) -> Vec<Breakpoint> {
        let breakpoints: Vec<SourceBreakpoint> = lines
            .iter()
            .map(|&line| SourceBreakpoint {
                line,
                column: None,
                condition: None,
                hit_condition: None,
                log_message: None,
            })
            .collect();

        #[allow(deprecated)]
        let seq = self
            .send(Command::SetBreakpoints(SetBreakpointsArguments {
                source: Source {
                    path: Some(path.to_string()),
                    name: None,
                    source_reference: None,
                    presentation_hint: None,
                    origin: None,
                    sources: None,
                    adapter_data: None,
                    checksums: None,
                },
                breakpoints: Some(breakpoints),
                lines: None,
                source_modified: None,
            }))
            .unwrap();

        let response = self.recv_response(seq);
        assert!(response.success, "SetBreakpoints request failed");

        match response.body {
            Some(ResponseBody::SetBreakpoints(sb)) => sb.breakpoints,
            other => panic!("Expected SetBreakpoints response body, got {:?}", other),
        }
    }

    /// Request the current stack trace.
    #[track_caller]
    pub fn stack_trace(&mut self) -> Vec<StackFrame> {
        let seq = self
            .send(Command::StackTrace(StackTraceArguments {
                thread_id: -1,
                start_frame: None,
                levels: None,
                format: None,
            }))
            .unwrap();

        let response = self.recv_response(seq);
        assert!(response.success, "StackTrace request failed");

        match response.body {
            Some(ResponseBody::StackTrace(st)) => st.stack_frames,
            other => panic!("Expected StackTrace response body, got {:?}", other),
        }
    }

    /// Request scopes for a stack frame.
    #[track_caller]
    pub fn scopes(&mut self, frame_id: i64) -> Vec<Scope> {
        let seq = self
            .send(Command::Scopes(ScopesArguments { frame_id }))
            .unwrap();

        let response = self.recv_response(seq);
        assert!(response.success, "Scopes request failed");

        match response.body {
            Some(ResponseBody::Scopes(s)) => s.scopes,
            other => panic!("Expected Scopes response body, got {:?}", other),
        }
    }

    /// Request variables for a given variables reference.
    #[track_caller]
    pub fn variables(&mut self, variables_reference: i64) -> Vec<Variable> {
        let seq = self
            .send(Command::Variables(VariablesArguments {
                variables_reference,
                filter: None,
                start: None,
                count: None,
                format: None,
            }))
            .unwrap();

        let response = self.recv_response(seq);
        assert!(response.success, "Variables request failed");

        match response.body {
            Some(ResponseBody::Variables(v)) => v.variables,
            other => panic!("Expected Variables response body, got {:?}", other),
        }
    }

    /// Request the list of threads.
    #[track_caller]
    pub fn threads(&mut self) -> Vec<Thread> {
        let seq = self.send(Command::Threads).unwrap();

        let response = self.recv_response(seq);
        assert!(response.success, "Threads request failed");

        match response.body {
            Some(ResponseBody::Threads(t)) => t.threads,
            other => panic!("Expected Threads response body, got {:?}", other),
        }
    }

    /// Disconnect from the DAP server.
    ///
    /// This method drains any pending events before expecting the disconnect response.
    pub fn disconnect(&mut self) {
        if !self.connected {
            return;
        }

        let seq = match self.send(Command::Disconnect(DisconnectArguments {
            restart: Some(false),
            terminate_debuggee: None,
            suspend_debuggee: None,
        })) {
            Ok(seq) => seq,
            Err(err) => {
                panic!("Failed to send Disconnect request: {err:?}");
            },
        };

        // Drain any pending events before expecting the response
        loop {
            let msg = match self.recv() {
                Ok(msg) => msg,
                Err(err) => {
                    panic!("Failed to receive DAP message during disconnect: {err:?}");
                },
            };

            match msg {
                Sendable::Response(response) => {
                    assert_eq!(
                        response.request_seq, seq,
                        "Response request_seq mismatch during disconnect"
                    );
                    assert!(response.success, "Disconnect request failed");
                    assert!(
                        matches!(response.body, Some(ResponseBody::Disconnect)),
                        "Expected Disconnect response body, got {:?}",
                        response.body
                    );
                    break;
                },
                Sendable::Event(_event) => {
                    // Events (like Continued) may arrive before the Disconnect
                    // response due to async processing. Drain them silently.
                    continue;
                },
                Sendable::ReverseRequest(req) => {
                    panic!("Unexpected ReverseRequest during disconnect: {:?}", req);
                },
            }
        }

        self.connected = false;
    }

    /// Send a DAP request. Returns the sequence number of the sent request.
    pub fn send(&mut self, command: Command) -> anyhow::Result<i64> {
        self.seq += 1;
        let request = Request {
            seq: self.seq,
            command,
        };

        let json = serde_json::to_string(&request)?;
        write!(
            self.writer,
            "Content-Length: {}\r\n\r\n{}",
            json.len(),
            json
        )?;
        self.writer.flush()?;

        Ok(self.seq)
    }

    /// Receive the next DAP message (response or event).
    ///
    /// Blocks until a message is received or the timeout expires.
    /// Returns a `Sendable` which can be matched to get `Response` or `Event`.
    pub fn recv(&mut self) -> anyhow::Result<Sendable> {
        // Read headers until we find Content-Length
        let mut content_length: Option<usize> = None;

        loop {
            let mut line = String::new();
            let bytes_read = self.reader.read_line(&mut line)?;

            if bytes_read == 0 {
                return Err(anyhow!("Connection closed"));
            }

            // Check for empty line (just \r\n or \n) which marks end of headers
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if content_length.is_some() {
                    // We have Content-Length and hit the empty separator line
                    break;
                }
                // Skip empty lines before headers (shouldn't happen but be safe)
                continue;
            }

            // Parse Content-Length header
            if let Some(value) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(value.trim().parse()?);
            }
            // Ignore other headers (like Content-Type)
        }

        let content_length =
            content_length.ok_or_else(|| anyhow!("Missing Content-Length header"))?;

        // Read the JSON content
        let mut content = vec![0u8; content_length];
        self.reader.read_exact(&mut content)?;

        let content = std::str::from_utf8(&content)?;
        let message: BaseMessage = serde_json::from_str(content)?;

        Ok(message.message)
    }

    /// Receive and assert the next message is a response to the given request.
    #[track_caller]
    pub fn recv_response(&mut self, request_seq: i64) -> Response {
        let msg = self.recv().expect("Failed to receive DAP message");
        match msg {
            Sendable::Response(response) => {
                assert_eq!(
                    response.request_seq, request_seq,
                    "Response request_seq mismatch"
                );
                response
            },
            Sendable::Event(event) => {
                panic!("Expected Response, got Event: {:?}", event);
            },
            Sendable::ReverseRequest(req) => {
                panic!("Expected Response, got ReverseRequest: {:?}", req);
            },
        }
    }

    /// Receive and assert the next message is an event.
    #[track_caller]
    pub fn recv_event(&mut self) -> Event {
        let msg = self.recv().expect("Failed to receive DAP message");
        match msg {
            Sendable::Event(event) => event,
            Sendable::Response(response) => {
                panic!("Expected Event, got Response: {:?}", response);
            },
            Sendable::ReverseRequest(req) => {
                panic!("Expected Event, got ReverseRequest: {:?}", req);
            },
        }
    }

    /// Receive and assert the next message is a Continued event.
    #[track_caller]
    pub fn recv_continued(&mut self) {
        let event = self.recv_event();
        assert!(
            matches!(event, Event::Continued(_)),
            "Expected Continued event, got {:?}",
            event
        );
    }

    /// Receive and assert the next message is a Stopped event with default fields.
    #[track_caller]
    pub fn recv_stopped(&mut self) {
        self.recv_stopped_impl(false);
    }

    /// Receive and assert the next message is a Stopped event with preserve_focus_hint set to true.
    ///
    /// This is expected when evaluating an expression in the debug console that
    /// doesn't change the debug position (e.g., inspecting a variable).
    #[track_caller]
    pub fn recv_stopped_preserve_focus(&mut self) {
        self.recv_stopped_impl(true);
    }

    #[track_caller]
    fn recv_stopped_impl(&mut self, preserve_focus: bool) {
        let event = self.recv_event();
        assert!(
            matches!(
                &event,
                Event::Stopped(StoppedEventBody {
                    reason: StoppedEventReason::Step,
                    description: None,
                    thread_id: Some(-1),
                    preserve_focus_hint: Some(pf),
                    text: None,
                    all_threads_stopped: Some(true),
                    hit_breakpoint_ids: None,
                }) if *pf == preserve_focus
            ),
            "Expected Stopped event with preserve_focus_hint={}, got {:?}",
            preserve_focus,
            event
        );
    }

    /// Receive and assert the next message is a Breakpoint event with verified=true.
    ///
    /// Returns the breakpoint from the event.
    #[track_caller]
    pub fn recv_breakpoint_verified(&mut self) -> Breakpoint {
        let event = self.recv_event();
        let Event::Breakpoint(BreakpointEventBody { breakpoint, .. }) = event else {
            panic!("Expected Breakpoint event, got {:?}", event);
        };
        assert!(
            breakpoint.verified,
            "Expected verified breakpoint, got {:?}",
            breakpoint
        );
        breakpoint
    }

    /// Receive a Breakpoint event and return the breakpoint.
    ///
    /// Does not assert on verified status.
    #[track_caller]
    pub fn recv_breakpoint_event(&mut self) -> Breakpoint {
        let event = self.recv_event();
        let Event::Breakpoint(BreakpointEventBody { breakpoint, .. }) = event else {
            panic!("Expected Breakpoint event, got {:?}", event);
        };
        breakpoint
    }

    /// Receive a Breakpoint event for an invalid breakpoint.
    ///
    /// Asserts that verified=false and message is present.
    #[track_caller]
    pub fn recv_breakpoint_invalid(&mut self) -> Breakpoint {
        let bp = self.recv_breakpoint_event();
        assert!(!bp.verified, "Expected unverified breakpoint, got {:?}", bp);
        assert!(
            bp.message.is_some(),
            "Expected message for invalid breakpoint, got {:?}",
            bp
        );
        bp
    }
}

impl Drop for DapClient {
    fn drop(&mut self) {
        // Don't try to disconnect if we're already panicking, as this could
        // obscure the original error
        if !std::thread::panicking() {
            self.disconnect();
        }
    }
}
