/*
 * dummy_frontend.rs
 *
 * Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
 *
 */

use assert_matches::assert_matches;
use serde_json::Value;

use crate::connection_file::ConnectionFile;
use crate::session::Session;
use crate::socket::socket::Socket;
use crate::wire::execute_input::ExecuteInput;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::jupyter_message::Status;
use crate::wire::status::ExecutionState;
use crate::wire::stream::Stream;
use crate::wire::wire_message::WireMessage;

pub struct DummyFrontend {
    pub _control_socket: Socket,
    pub shell_socket: Socket,
    pub iopub_socket: Socket,
    pub stdin_socket: Socket,
    pub heartbeat_socket: Socket,
    session: Session,
    key: String,
    control_port: u16,
    shell_port: u16,
    iopub_port: u16,
    stdin_port: u16,
    heartbeat_port: u16,
}

impl DummyFrontend {
    pub fn new() -> Self {
        use rand::Rng;

        // Create a random HMAC key for signing messages.
        let key_bytes = rand::thread_rng().gen::<[u8; 16]>();
        let key = hex::encode(key_bytes);

        // Create a random socket identity for the shell and stdin sockets. Per
        // the Jupyter specification, these must share a ZeroMQ identity.
        let shell_id = rand::thread_rng().gen::<[u8; 16]>();

        // Create a new kernel session from the key
        let session = Session::create(key.clone()).unwrap();

        let ctx = zmq::Context::new();

        let control_port = portpicker::pick_unused_port().unwrap();
        let control = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Control"),
            zmq::DEALER,
            None,
            format!("tcp://127.0.0.1:{}", control_port),
        )
        .unwrap();

        let shell_port = portpicker::pick_unused_port().unwrap();
        let shell = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Shell"),
            zmq::DEALER,
            Some(&shell_id),
            format!("tcp://127.0.0.1:{}", shell_port),
        )
        .unwrap();

        let iopub_port = portpicker::pick_unused_port().unwrap();
        let iopub = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("IOPub"),
            zmq::SUB,
            None,
            format!("tcp://127.0.0.1:{}", iopub_port),
        )
        .unwrap();

        let stdin_port = portpicker::pick_unused_port().unwrap();
        let stdin = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Stdin"),
            zmq::DEALER,
            Some(&shell_id),
            format!("tcp://127.0.0.1:{}", stdin_port),
        )
        .unwrap();

        let heartbeat_port = portpicker::pick_unused_port().unwrap();
        let heartbeat = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Heartbeat"),
            zmq::REQ,
            None,
            format!("tcp://127.0.0.1:{}", heartbeat_port),
        )
        .unwrap();

        Self {
            session,
            key,
            control_port,
            _control_socket: control,
            shell_port,
            shell_socket: shell,
            iopub_port,
            iopub_socket: iopub,
            stdin_port,
            stdin_socket: stdin,
            heartbeat_port,
            heartbeat_socket: heartbeat,
        }
    }

    /// Completes initialization of the frontend (usually done after the kernel
    /// is ready and connected)
    pub fn complete_initialization(&self) {
        self.iopub_socket.subscribe().unwrap();
    }

    /// Sends a Jupyter message on the Shell socket; returns the ID of the newly
    /// created message
    pub fn send_shell<T: ProtocolMessage>(&self, msg: T) -> String {
        let message = JupyterMessage::create(msg, None, &self.session);
        let id = message.header.msg_id.clone();
        message.send(&self.shell_socket).unwrap();
        id
    }

    pub fn send_execute_request(&self, code: &str) -> String {
        self.send_shell(ExecuteRequest {
            code: String::from(code),
            silent: false,
            store_history: true,
            user_expressions: serde_json::Value::Null,
            allow_stdin: false,
            stop_on_error: false,
        })
    }

    /// Sends a Jupyter message on the Stdin socket
    pub fn send_stdin<T: ProtocolMessage>(&self, msg: T) {
        let message = JupyterMessage::create(msg, None, &self.session);
        message.send(&self.stdin_socket).unwrap();
    }

    pub fn recv(&self, socket: &Socket) -> Message {
        // It's important to wait with a timeout because the kernel thread might
        // have panicked, preventing it from sending the expected message. The
        // tests would then hang indefinitely.
        //
        // Note that the panic hook will still have run to record the panic, so
        // we'll get expected panic information in the test output.
        if socket.poll_incoming(1000).unwrap() {
            return Message::read_from_socket(socket).unwrap();
        }

        panic!("Timeout while expecting message on socket {}", socket.name);
    }

    /// Receives a Jupyter message from the Shell socket
    pub fn recv_shell(&self) -> Message {
        self.recv(&self.shell_socket)
    }

    /// Receives a Jupyter message from the IOPub socket
    pub fn recv_iopub(&self) -> Message {
        self.recv(&self.iopub_socket)
    }

    /// Receives a Jupyter message from the Stdin socket
    pub fn recv_stdin(&self) -> Message {
        self.recv(&self.stdin_socket)
    }

    /// Receive from Shell and assert `ExecuteReply` message.
    /// Returns `execution_count`.
    pub fn recv_shell_execute_reply(&self) -> u32 {
        let msg = self.recv_shell();

        assert_matches!(msg, Message::ExecuteReply(data) => {
            assert_eq!(data.content.status, Status::Ok);
            data.content.execution_count
        })
    }

    /// Receive from Shell and assert `ExecuteReplyException` message.
    /// Returns `execution_count`.
    pub fn recv_shell_execute_reply_exception(&self) -> u32 {
        let msg = self.recv_shell();

        assert_matches!(msg, Message::ExecuteReplyException(data) => {
            assert_eq!(data.content.status, Status::Error);
            data.content.execution_count
        })
    }

    /// Receive from IOPub and assert Busy message
    pub fn recv_iopub_busy(&self) -> () {
        let msg = self.recv_iopub();

        assert_matches!(msg, Message::Status(data) => {
            assert_eq!(data.content.execution_state, ExecutionState::Busy);
        });
    }

    /// Receive from IOPub and assert Idle message
    pub fn recv_iopub_idle(&self) -> () {
        let msg = self.recv_iopub();

        assert_matches!(msg, Message::Status(data) => {
            assert_eq!(data.content.execution_state, ExecutionState::Idle);
        });
    }

    /// Receive from IOPub and assert ExecuteInput message
    pub fn recv_iopub_execute_input(&self) -> ExecuteInput {
        let msg = self.recv_iopub();

        assert_matches!(msg, Message::ExecuteInput(data) => {
            data.content
        })
    }

    /// Receive from IOPub and assert ExecuteResult message. Returns compulsory
    /// `plain/text` result.
    pub fn recv_iopub_execute_result(&self) -> String {
        let msg = self.recv_iopub();

        assert_matches!(msg, Message::ExecuteResult(data) => {
            assert_matches!(data.content.data, Value::Object(map) => {
                assert_matches!(map["text/plain"], Value::String(ref string) => {
                    string.clone()
                })
            })
        })
    }

    pub fn recv_iopub_stream_stdout(&self) -> String {
        let msg = self.recv_iopub();

        assert_matches!(msg, Message::StreamOutput(data) => {
            assert_eq!(data.content.name, Stream::Stdout);
            data.content.text
        })
    }

    pub fn recv_iopub_stream_stderr(&self) -> String {
        let msg = self.recv_iopub();

        assert_matches!(msg, Message::StreamOutput(data) => {
            assert_eq!(data.content.name, Stream::Stderr);
            data.content.text
        })
    }

    /// Receive from IOPub and assert ExecuteResult message. Returns compulsory
    /// `evalue` field.
    pub fn recv_iopub_execute_error(&self) -> String {
        let msg = self.recv_iopub();

        assert_matches!(msg, Message::ExecuteError(data) => {
            data.content.exception.evalue
        })
    }

    /// Receives a (raw) message from the heartbeat socket
    pub fn recv_heartbeat(&self) -> zmq::Message {
        let mut msg = zmq::Message::new();
        self.heartbeat_socket.recv(&mut msg).unwrap();
        msg
    }

    /// Sends a (raw) message to the heartbeat socket
    pub fn send_heartbeat(&self, msg: zmq::Message) {
        self.heartbeat_socket.send(msg).unwrap();
    }

    /// Gets a connection file for the Amalthea kernel that will connect it to
    /// this synthetic frontend.
    pub fn get_connection_file(&self) -> ConnectionFile {
        ConnectionFile {
            control_port: self.control_port,
            shell_port: self.shell_port,
            stdin_port: self.stdin_port,
            iopub_port: self.iopub_port,
            hb_port: self.heartbeat_port,
            transport: String::from("tcp"),
            signature_scheme: String::from("hmac-sha256"),
            ip: String::from("127.0.0.1"),
            key: self.key.clone(),
        }
    }

    /// Asserts that no socket has incoming data
    pub fn assert_no_incoming(&mut self) {
        let mut has_incoming = false;

        if self.iopub_socket.has_incoming_data().unwrap() {
            has_incoming = true;
            Self::flush_incoming("IOPub", &self.iopub_socket);
        }
        if self.shell_socket.has_incoming_data().unwrap() {
            has_incoming = true;
            Self::flush_incoming("Shell", &self.shell_socket);
        }
        if self.stdin_socket.has_incoming_data().unwrap() {
            has_incoming = true;
            Self::flush_incoming("StdIn", &self.stdin_socket);
        }
        if self.heartbeat_socket.has_incoming_data().unwrap() {
            has_incoming = true;
            Self::flush_incoming("Heartbeat", &self.heartbeat_socket);
        }

        if has_incoming {
            panic!("Sockets must be empty on exit (see details above)");
        }
    }

    fn flush_incoming(name: &str, socket: &Socket) {
        println!("{name} has incoming data:");

        while socket.has_incoming_data().unwrap() {
            dbg!(WireMessage::read_from_socket(socket).unwrap());
            println!("---");
        }
    }
}
