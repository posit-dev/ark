/*
 * signed_socket.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::session::Session;
use log::trace;
use std::sync::{Arc, Mutex};

/// Represents a socket that sends and receives messages that are optionally
/// signed with a SHA-256 HMAC.
pub struct SignedSocket {
    /// A ZeroMQ socket over which signed messages are to be sent/received
    pub socket: Arc<Mutex<zmq::Socket>>,

    /// The Jupyter session information associated with the socket, including
    /// the session ID and HMAC signing key
    pub session: Session,

    name: String,
}

impl SignedSocket {
    pub fn new(
        session: Session,
        ctx: zmq::Context,
        name: String,
        kind: zmq::SocketType,
        endpoint: String,
    ) -> Result<Self, Error> {
        let socket = match ctx.socket(kind) {
            Ok(s) => s,
            Err(err) => return Err(Error::CreateSocketFailed(name, err)),
        };
        trace!("Binding to ZeroMQ '{}' socket at {}", name, endpoint);
        if let Err(err) = socket.bind(&endpoint) {
            return Err(Error::SocketBindError(name, endpoint, err));
        }
        Ok(Self {
            socket: Arc::new(Mutex::new(socket)),
            session: session,
            name: name,
        })
    }

    pub fn recv_multipart(&self) -> Result<Vec<Vec<u8>>, Error> {
        match self.socket.lock() {
            Ok(socket) => match socket.recv_multipart(0) {
                Ok(data) => Ok(data),
                Err(err) => Err(Error::ZmqError(self.name, err)),
            },
            Err(err) => Err(Error::CannotLockSocket(self.name)),
        }
    }

    pub fn send_multipart(&self, data: Vec<Vec<u8>>) -> Result<(), Error>

    pub fn send(&self) -> Result<(), Error> {}
}
