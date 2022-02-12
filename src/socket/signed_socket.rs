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
