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
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

pub struct Frontend {
    session: Session,
    receiver: Receiver<Message>,
    key: String,
    control_socket: Socket,
    shell_socket: Socket,
    iopub_socket: Socket,
    stdin_socket: Socket,
    heartbeat_socket: Socket,
}

impl Frontend {
    pub fn new() -> Self {
        use rand::Rng;

        // Create a random HMAC key for signing messages.
        let key_bytes = rand::thread_rng().gen::<[u8; 16]>();
        let key = hex::encode(key_bytes);

        // Create a new kernel session from the key
        let session = Session::create(key.clone()).unwrap();

        // Create an MPSC channel for receiving kernel messages.
        let (sender, receiver) = channel::<Message>();

        let ctx = zmq::Context::new();

        let control = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Control"),
            zmq::DEALER,
            String::from("tcp://127.0.0.1:8080/"),
        )
        .unwrap();
        let control_socket = control.clone();
        let control_sender = sender.clone();
        thread::spawn(move || Self::message_proxy_thread(control_socket, control_sender));

        let shell = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Shell"),
            zmq::DEALER,
            String::from("tcp://127.0.0.1:8081/"),
        )
        .unwrap();
        let shell_socket = shell.clone();
        let shell_sender = sender.clone();
        thread::spawn(move || Self::message_proxy_thread(shell_socket, shell_sender));

        let iopub = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("IOPub"),
            zmq::PUB,
            String::from("tcp://127.0.0.1:8082/"),
        )
        .unwrap();
        let iopub_socket = iopub.clone();
        let iopub_sender = sender.clone();
        thread::spawn(move || Self::message_proxy_thread(iopub_socket, iopub_sender));

        let stdin = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Stdin"),
            zmq::DEALER,
            String::from("tcp://127.0.0.1:8083/"),
        )
        .unwrap();
        let stdin_socket = stdin.clone();
        let stdin_sender = sender.clone();
        thread::spawn(move || Self::message_proxy_thread(stdin_socket, stdin_sender));

        let heartbeat = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Heartbeat"),
            zmq::REQ,
            String::from("tcp://127.0.0.1:8084/"),
        )
        .unwrap();

        Self {
            session: session,
            receiver: receiver,
            key: key,
            control_socket: control,
            shell_socket: shell,
            iopub_socket: iopub,
            stdin_socket: stdin,
            heartbeat_socket: heartbeat,
        }
    }

    /// Receives and returns the next message from the kernel (from any socket)
    pub fn receive(&self) -> Message {
        self.receiver.recv().unwrap()
    }

    /// Sends a message on the Shell socket
    pub fn send_shell<T: ProtocolMessage>(&self, msg: T) {
        let message = JupyterMessage::create(msg, None, &self.session);
        message.send(&self.shell_socket).unwrap();
    }

    pub fn get_connection_file(&self) -> ConnectionFile {
        ConnectionFile {
            control_port: 8080,
            shell_port: 8081,
            stdin_port: 8082,
            iopub_port: 8083,
            hb_port: 8084,
            transport: String::from("tcp"),
            signature_scheme: String::from("hmac-sha256"),
            ip: String::from("127.0.0.1"),
            key: self.key.clone(),
        }
    }

    /// Runs on a thread to accept messages from a ZeroMQ socket connected to
    /// the kernel and funnel them into an MPSC channel.
    fn message_proxy_thread(socket: Socket, sender: Sender<Message>) {
        loop {
            let message = match Message::read_from_socket(&socket) {
                Ok(m) => m,
                Err(err) => {
                    panic!("Could not read message from socket proxy: {}", err);
                }
            };
            sender.send(message).unwrap();
        }
    }
}
