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
use dap::events::Event;
use dap::requests::AttachRequestArguments;
use dap::requests::Command;
use dap::requests::ContinueArguments;
use dap::requests::DisconnectArguments;
use dap::requests::InitializeArguments;
use dap::requests::Request;
use dap::responses::Response;
use dap::responses::ResponseBody;
use dap::types::Capabilities;

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
                lines_start_at1: Some(true),
                columns_start_at1: Some(true),
                ..Default::default()
            }))
            .expect("Failed to send Initialize request");

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
            .expect("Failed to send Attach request");

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
            .expect("Failed to send Continue request");

        let response = self.recv_response(seq);
        assert!(response.success, "Continue request failed");
        assert!(
            matches!(response.body, Some(ResponseBody::Continue(_))),
            "Expected Continue response body, got {:?}",
            response.body
        );
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

    /// Receive and assert the next message is a Stopped event.
    #[track_caller]
    pub fn recv_stopped(&mut self) {
        let event = self.recv_event();
        assert!(
            matches!(event, Event::Stopped(_)),
            "Expected Stopped event, got {:?}",
            event
        );
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
