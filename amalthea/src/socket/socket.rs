/*
 * socket.rs
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
///
/// Internally, it wraps an ZeroMQ socket with a mutex, so Socket objects can be
/// shared in a threadsafe way.
#[derive(Clone)]
pub struct Socket {
    /// The Jupyter session information associated with the socket, including
    /// the session ID and HMAC signing key
    pub session: Session,

    /// The name of the socket; used only to give context to debugging/trace
    /// messages
    pub name: String,

    /// A ZeroMQ socket over which signed messages are to be sent/received
    socket: Arc<Mutex<zmq::Socket>>,
}

impl Socket {
    /// Create a new Socket instance from a kernel session and a ZeroMQ context.
    pub fn new(
        session: Session,
        ctx: zmq::Context,
        name: String,
        kind: zmq::SocketType,
        endpoint: String,
    ) -> Result<Self, Error> {
        // Create the underlying ZeroMQ socket
        let socket = match ctx.socket(kind) {
            Ok(s) => s,
            Err(err) => return Err(Error::CreateSocketFailed(name, err)),
        };

        // Bind the socket to the requested endpoint
        trace!("Binding to ZeroMQ '{}' socket at {}", name, endpoint);
        if let Err(err) = socket.bind(&endpoint) {
            return Err(Error::SocketBindError(name, endpoint, err));
        }

        // Create a new mutex and return
        Ok(Self {
            socket: Arc::new(Mutex::new(socket)),
            session: session,
            name: name,
        })
    }

    /// Receive a message from the socket.
    ///
    /// **Note**: This will block until the socket is available, and block again
    /// until a message is delivered on the socket.
    pub fn recv(&self, msg: &mut zmq::Message) -> Result<(), Error> {
        match self.socket.lock() {
            Ok(socket) => {
                if let Err(err) = socket.recv(msg, 0) {
                    Err(Error::ZmqError(self.name.clone(), err))
                } else {
                    Ok(())
                }
            }
            Err(_) => Err(Error::CannotLockSocket(
                self.name.clone(),
                String::from("message send"),
            )),
        }
    }

    /// Receive a multi-part message from the socket.
    ///
    /// **Note**: This will block until the socket is available, and block again
    /// until a message is delivered on the socket.
    pub fn recv_multipart(&self) -> Result<Vec<Vec<u8>>, Error> {
        match self.socket.lock() {
            Ok(socket) => match socket.recv_multipart(0) {
                Ok(data) => Ok(data),
                Err(err) => Err(Error::ZmqError(self.name.clone(), err)),
            },
            Err(_) => Err(Error::CannotLockSocket(
                self.name.clone(),
                String::from("multipart receive"),
            )),
        }
    }

    /// Send a message on the socket.
    ///
    /// **Note**: This will block until the socket is available.
    pub fn send(&self, msg: zmq::Message) -> Result<(), Error> {
        match self.socket.lock() {
            Ok(socket) => match socket.send(msg, 0) {
                Ok(data) => Ok(data),
                Err(err) => Err(Error::ZmqError(self.name.clone(), err)),
            },
            Err(_) => Err(Error::CannotLockSocket(
                self.name.clone(),
                String::from("message send"),
            )),
        }
    }

    /// Send a multi-part message on the socket.
    ///
    /// **Note**: This will block until the socket is available.
    pub fn send_multipart(&self, data: &Vec<Vec<u8>>) -> Result<(), Error> {
        match self.socket.lock() {
            Ok(socket) => match socket.send_multipart(data, 0) {
                Ok(data) => Ok(data),
                Err(err) => Err(Error::ZmqError(self.name.clone(), err)),
            },
            Err(_) => Err(Error::CannotLockSocket(
                self.name.clone(),
                String::from("multipart send"),
            )),
        }
    }
}
