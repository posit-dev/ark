/*
 * mod.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use amalthea::connection_file::ConnectionFile;
use amalthea::session::Session;
use amalthea::socket::socket::Socket;

pub struct Frontend {
    session: Session,
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
        let session = Session::create(key).unwrap();

        let ctx = zmq::Context::new();

        let control = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Control"),
            zmq::DEALER,
            String::from("tcp://127.0.0.1:8080/"),
        )
        .unwrap();

        let shell = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Shell"),
            zmq::DEALER,
            String::from("tcp://127.0.0.1:8081/"),
        )
        .unwrap();

        let iopub = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("IOPub"),
            zmq::PUB,
            String::from("tcp://127.0.0.1:8082/"),
        )
        .unwrap();

        let stdin = Socket::new(
            session.clone(),
            ctx.clone(),
            String::from("Stdin"),
            zmq::DEALER,
            String::from("tcp://127.0.0.1:8083/"),
        )
        .unwrap();

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
            control_socket: control,
            shell_socket: shell,
            iopub_socket: iopub,
            stdin_socket: stdin,
            heartbeat_socket: heartbeat,
        }
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
            key: String::from(""), // TODO: generate this!
        }
    }
}
