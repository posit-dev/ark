/*
 * mod.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::connection_file::ConnectionFile;
use amalthea::session::Session;
use amalthea::socket::socket::Socket;
use amalthea::wire::jupyter_message::{JupyterMessage, Message, ProtocolMessage};

pub struct Frontend {
    pub control_socket: Socket,
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

impl Frontend {
    pub fn new() -> Self {
        use rand::Rng;

        // Create a random HMAC key for signing messages.
        let key_bytes = rand::thread_rng().gen::<[u8; 16]>();
        let key = hex::encode(key_bytes);

        // Create a new kernel session from the key
        let session = Session::create(key.clone()).unwrap();

        let ctx = zmq::Context::new();

        let control_port = portpicker::pick_unused_port().unwrap();
        let control = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Control"),
            zmq::DEALER,
            format!("tcp://127.0.0.1:{}", control_port),
        )
        .unwrap();

        let shell_port = portpicker::pick_unused_port().unwrap();
        let shell = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Shell"),
            zmq::DEALER,
            format!("tcp://127.0.0.1:{}", shell_port),
        )
        .unwrap();

        let iopub_port = portpicker::pick_unused_port().unwrap();
        let iopub = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("IOPub"),
            zmq::SUB,
            format!("tcp://127.0.0.1:{}", iopub_port),
        )
        .unwrap();

        let stdin_port = portpicker::pick_unused_port().unwrap();
        let stdin = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Stdin"),
            zmq::DEALER,
            format!("tcp://127.0.0.1:{}", stdin_port),
        )
        .unwrap();

        let heartbeat_port = portpicker::pick_unused_port().unwrap();
        let heartbeat = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Heartbeat"),
            zmq::REQ,
            format!("tcp://127.0.0.1:{}", heartbeat_port),
        )
        .unwrap();

        Self {
            session,
            key,
            control_port,
            control_socket: control,
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

    /// Sends a message on the Shell socket
    pub fn send_shell<T: ProtocolMessage>(&self, msg: T) {
        let message = JupyterMessage::create(msg, None, &self.session);
        message.send(&self.shell_socket).unwrap();
    }

    /// Receives a message from the Shell socket
    pub fn receive_shell(&self) -> Message {
        Message::read_from_socket(&self.shell_socket).unwrap()
    }

    /// Receives a message from the IOPub socket
    pub fn receive_iopub(&self) -> Message {
        Message::read_from_socket(&self.iopub_socket).unwrap()
    }

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
}
