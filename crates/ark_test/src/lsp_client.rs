//
// lsp_client.rs
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

use serde_json::json;
use serde_json::Value;
use tower_lsp::lsp_types;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A minimal LSP client for integration tests.
///
/// Communicates with the LSP server over TCP using the standard
/// `Content-Length` framed JSON-RPC 2.0 protocol.
///
/// Automatically shuts down when dropped.
pub struct LspClient {
    reader: BufReader<TcpStream>,
    writer: BufWriter<TcpStream>,
    next_id: i64,
    initialized: bool,
    /// Documents opened by this client, closed on drop
    open_documents: Vec<lsp_types::Url>,
    /// Server capabilities from the initialize response
    server_capabilities: Option<lsp_types::ServerCapabilities>,
    /// Buffered diagnostics notifications, keyed by document URI
    diagnostics: std::collections::HashMap<lsp_types::Url, Vec<lsp_types::Diagnostic>>,
}

impl LspClient {
    /// Connect to an LSP server at the given address and port.
    pub fn connect(addr: &str, port: u16) -> anyhow::Result<Self> {
        let stream = TcpStream::connect(format!("{addr}:{port}"))?;
        stream.set_read_timeout(Some(DEFAULT_TIMEOUT))?;
        stream.set_write_timeout(Some(DEFAULT_TIMEOUT))?;

        let reader = BufReader::new(stream.try_clone()?);
        let writer = BufWriter::new(stream);

        Ok(Self {
            reader,
            writer,
            next_id: 0,
            initialized: false,
            open_documents: Vec::new(),
            server_capabilities: None,
            diagnostics: std::collections::HashMap::new(),
        })
    }

    /// Initialize the LSP session.
    ///
    /// Sends the `initialize` request followed by the `initialized` notification.
    pub fn initialize(&mut self) -> lsp_types::InitializeResult {
        let response = self.send_request(
            "initialize",
            json!({
                "capabilities": {}
            }),
        );

        let result: lsp_types::InitializeResult = serde_json::from_value(response).unwrap();

        self.send_notification("initialized", json!({}));

        // The server sends `client/registerCapability` after `initialized`
        self.recv_server_request("client/registerCapability");

        self.initialized = true;
        self.server_capabilities = Some(result.capabilities.clone());

        result
    }

    /// Returns the server capabilities from the initialize response.
    pub fn server_capabilities(&self) -> &lsp_types::ServerCapabilities {
        self.server_capabilities
            .as_ref()
            .expect("LSP client not initialized")
    }

    /// Returns diagnostics for a document, if any have been received.
    pub fn diagnostics(&self, uri: &lsp_types::Url) -> Option<&Vec<lsp_types::Diagnostic>> {
        self.diagnostics.get(uri)
    }

    /// Clears buffered diagnostics for a document.
    pub fn clear_diagnostics(&mut self, uri: &lsp_types::Url) {
        self.diagnostics.remove(uri);
    }

    /// Notify the server that a document was opened.
    ///
    /// Returns the URI assigned to the document (based on the provided `name`).
    pub fn open_document(&mut self, name: &str, text: &str) -> lsp_types::Url {
        let uri = lsp_types::Url::parse(&format!("file:///test/{name}")).unwrap();

        let params = lsp_types::DidOpenTextDocumentParams {
            text_document: lsp_types::TextDocumentItem {
                uri: uri.clone(),
                language_id: String::from("r"),
                version: 0,
                text: text.to_string(),
            },
        };

        self.send_notification(
            "textDocument/didOpen",
            serde_json::to_value(params).unwrap(),
        );

        self.open_documents.push(uri.clone());
        uri
    }

    /// Close a previously opened document.
    pub fn close_document(&mut self, uri: &lsp_types::Url) {
        let params = lsp_types::DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
        };

        self.send_notification(
            "textDocument/didClose",
            serde_json::to_value(params).unwrap(),
        );

        self.open_documents.retain(|u| u != uri);
    }

    /// Request completions at the given 0-based line and character position.
    pub fn completions(
        &mut self,
        uri: &lsp_types::Url,
        line: u32,
        character: u32,
    ) -> Vec<lsp_types::CompletionItem> {
        let params = lsp_types::CompletionParams {
            text_document_position: lsp_types::TextDocumentPositionParams {
                text_document: lsp_types::TextDocumentIdentifier { uri: uri.clone() },
                position: lsp_types::Position { line, character },
            },
            work_done_progress_params: lsp_types::WorkDoneProgressParams::default(),
            partial_result_params: lsp_types::PartialResultParams::default(),
            context: None,
        };

        let response = self.send_request(
            "textDocument/completion",
            serde_json::to_value(params).unwrap(),
        );

        if response.is_null() {
            return Vec::new();
        }

        let completion_response: lsp_types::CompletionResponse =
            serde_json::from_value(response).unwrap();

        match completion_response {
            lsp_types::CompletionResponse::Array(items) => items,
            lsp_types::CompletionResponse::List(list) => list.items,
        }
    }

    /// Send a JSON-RPC request and wait for the response.
    ///
    /// Returns the `result` field from the response.
    ///
    /// # Panics
    ///
    /// Panics if the response contains an error.
    #[track_caller]
    pub fn send_request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;

        let mut message = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
        });
        if !params.is_null() {
            message["params"] = params;
        }

        self.send_raw(&message);
        self.recv_response(method, id)
    }

    /// Send a JSON-RPC notification (no response expected).
    pub fn send_notification(&mut self, method: &str, params: Value) {
        let mut message = json!({
            "jsonrpc": "2.0",
            "method": method,
        });
        if !params.is_null() {
            message["params"] = params;
        }

        self.send_raw(&message);
    }

    /// Send the `shutdown` request followed by the `exit` notification.
    pub fn shutdown(&mut self) {
        if !self.initialized {
            return;
        }

        // Close any open documents first
        let uris: Vec<lsp_types::Url> = std::mem::take(&mut self.open_documents);
        for uri in &uris {
            self.close_document(uri);
        }

        let _ = self.send_request("shutdown", Value::Null);
        self.send_notification("exit", Value::Null);
        self.initialized = false;
    }

    fn send_raw(&mut self, message: &Value) {
        let body = serde_json::to_string(message).unwrap();
        write!(
            self.writer,
            "Content-Length: {}\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
        self.writer.flush().unwrap();
    }

    /// Receive JSON-RPC messages until we get the response matching `id`.
    ///
    /// Buffers `publishDiagnostics` notifications for later assertion via
    /// `diagnostics()`. Panics on unexpected messages.
    #[track_caller]
    fn recv_response(&mut self, method: &str, id: i64) -> Value {
        loop {
            match self.recv_any() {
                LspMessage::Response {
                    id: msg_id,
                    result,
                    error,
                } => {
                    assert_eq!(
                        msg_id, id,
                        "Response id mismatch: expected {id}, got {msg_id}"
                    );
                    if let Some(error) = error {
                        panic!("LSP error response for `{method}`: {error}");
                    }
                    return result.unwrap_or(Value::Null);
                },
                LspMessage::ServerRequest {
                    method: req_method, ..
                } => {
                    panic!(
                        "Unexpected LSP server request `{req_method}` while waiting for response to `{method}`"
                    );
                },
                LspMessage::Notification { diagnostics } => {
                    if let Some(params) = diagnostics {
                        self.diagnostics.insert(params.uri, params.diagnostics);
                    }
                },
            }
        }
    }

    /// Receive the next server-to-client request, assert its method, and
    /// auto-reply with an empty success so the server doesn't block.
    ///
    /// Skips benign server notifications. Panics on unexpected messages.
    #[track_caller]
    fn recv_server_request(&mut self, expected_method: &str) {
        loop {
            match self.recv_any() {
                LspMessage::ServerRequest { id, method } => {
                    assert_eq!(
                        method, expected_method,
                        "Expected server request `{expected_method}`, got `{method}`"
                    );
                    let response = json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": null,
                    });
                    self.send_raw(&response);
                    return;
                },
                LspMessage::Notification { diagnostics } => {
                    if let Some(params) = diagnostics {
                        self.diagnostics.insert(params.uri, params.diagnostics);
                    }
                },
                other => {
                    panic!("Expected server request `{expected_method}`, got: {other:?}");
                },
            }
        }
    }
}

#[derive(Debug)]
enum LspMessage {
    Response {
        id: i64,
        result: Option<Value>,
        error: Option<Value>,
    },
    ServerRequest {
        id: Value,
        method: String,
    },
    Notification {
        diagnostics: Option<lsp_types::PublishDiagnosticsParams>,
    },
}

impl LspClient {
    /// Read one JSON-RPC message and classify it, checking notifications
    /// for errors along the way.
    fn recv_any(&mut self) -> LspMessage {
        let message = self.recv_message().expect("Failed to receive LSP message");
        let has_id = message.contains_key("id");
        let has_method = message.contains_key("method");
        let has_result = message.contains_key("result") || message.contains_key("error");

        match (has_id, has_method, has_result) {
            (true, false, true) => LspMessage::Response {
                id: message["id"].as_i64().unwrap(),
                result: message.get("result").cloned(),
                error: message.get("error").cloned(),
            },

            (true, true, false) => LspMessage::ServerRequest {
                id: message["id"].clone(),
                method: message["method"].as_str().unwrap_or("unknown").to_string(),
            },

            (false, true, false) => {
                let diagnostics = Self::check_server_notification(&message);
                LspMessage::Notification { diagnostics }
            },

            _ => panic!("Unrecognised LSP message shape: {message:?}"),
        }
    }

    /// Check a server notification, returning parsed diagnostics if applicable.
    fn check_server_notification(
        message: &serde_json::Map<String, Value>,
    ) -> Option<lsp_types::PublishDiagnosticsParams> {
        let method = message["method"].as_str().unwrap_or("unknown");
        match method {
            "window/logMessage" => {
                // Surface LSP server errors and warnings as test
                // failures so they don't go unnoticed.
                // MessageType: 1 = Error, 2 = Warning, 3 = Info, 4 = Log
                let msg_type = message["params"]["type"].as_u64().unwrap_or(0);
                if msg_type <= 2 {
                    let level = if msg_type == 1 { "error" } else { "warning" };
                    let text = message["params"]["message"]
                        .as_str()
                        .unwrap_or("(no message)");
                    panic!("LSP server {level}: {text}");
                }
                None
            },
            "textDocument/publishDiagnostics" => {
                let params: lsp_types::PublishDiagnosticsParams =
                    serde_json::from_value(message["params"].clone())
                        .expect("Failed to parse publishDiagnostics params");
                Some(params)
            },
            _ => panic!("Unexpected LSP notification `{method}`: {message:?}"),
        }
    }

    /// Read one JSON-RPC message from the stream.
    fn recv_message(&mut self) -> anyhow::Result<serde_json::Map<String, Value>> {
        let mut content_length: Option<usize> = None;

        loop {
            let mut line = String::new();
            let bytes_read = self.reader.read_line(&mut line)?;

            if bytes_read == 0 {
                return Err(anyhow::anyhow!("LSP connection closed unexpectedly"));
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                if content_length.is_some() {
                    break;
                }
                continue;
            }

            if let Some(value) = trimmed.strip_prefix("Content-Length:") {
                content_length = Some(value.trim().parse()?);
            }
        }

        let content_length =
            content_length.ok_or_else(|| anyhow::anyhow!("Missing Content-Length header"))?;

        let mut buf = vec![0u8; content_length];
        self.reader.read_exact(&mut buf)?;

        let text = std::str::from_utf8(&buf)?;
        let message: serde_json::Map<String, Value> = serde_json::from_str(text)?;

        Ok(message)
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        self.shutdown();
    }
}
