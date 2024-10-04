/*
 * socket.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use log::trace;

use crate::error::Error;
use crate::session::Session;

/// Represents a socket that sends and receives messages that are optionally
/// signed with a SHA-256 HMAC.
pub struct Socket {
    /// The Jupyter session information associated with the socket, including
    /// the session ID and HMAC signing key
    pub session: Session,

    /// The name of the socket; used only to give context to debugging/trace
    /// messages
    pub name: String,

    /// A ZeroMQ socket over which signed messages are to be sent/received
    pub socket: zmq::Socket,
}

impl Socket {
    /// Create a new Socket instance from a kernel session and a ZeroMQ context.
    pub fn new(
        session: Session,
        ctx: zmq::Context,
        name: String,
        kind: zmq::SocketType,
        identity: Option<&[u8]>,
        endpoint: String,
    ) -> Result<Self, Error> {
        let socket = Self::new_raw(ctx, name.clone(), kind, identity)?;

        // One side of a socket must `bind()` to its endpoint, and the other
        // side must `connect()` to the same endpoint. The `bind()` side
        // will be the server, and the `connect()` side will be the client.
        match kind {
            zmq::SocketType::ROUTER | zmq::SocketType::PUB | zmq::SocketType::REP => {
                trace!("Binding to ZeroMQ '{}' socket at {}", name, endpoint);
                if let Err(err) = socket.bind(&endpoint) {
                    return Err(Error::SocketBindError(name, endpoint, err));
                }
            },
            zmq::SocketType::DEALER | zmq::SocketType::SUB | zmq::SocketType::REQ => {
                // Bind the socket to the requested endpoint
                trace!("Connecting to ZeroMQ '{}' socket at {}", name, endpoint);
                if let Err(err) = socket.connect(&endpoint) {
                    return Err(Error::SocketConnectError(name, endpoint, err));
                }
            },
            _ => return Err(Error::UnsupportedSocketType(kind)),
        }

        // If this is a debug build, set `ZMQ_ROUTER_MANDATORY` on all `ROUTER`
        // sockets, so that we get errors instead of silent message drops for
        // unroutable messages.
        #[cfg(debug_assertions)]
        {
            if kind == zmq::ROUTER {
                if let Err(err) = socket.set_router_mandatory(true) {
                    return Err(Error::SocketBindError(name, endpoint, err));
                }
            }
        }

        // Create a new mutex and return
        Ok(Self {
            socket,
            session,
            name,
        })
    }

    pub fn new_pair(
        session: Session,
        ctx: zmq::Context,
        name: String,
        identity: Option<&[u8]>,
        endpoint: String,
        bind: bool,
    ) -> Result<Self, Error> {
        let socket = Self::new_raw(ctx, name.clone(), zmq::PAIR, identity)?;

        if bind {
            trace!("Binding to ZeroMQ '{}' socket at {}", name, endpoint);
            if let Err(err) = socket.bind(&endpoint) {
                return Err(Error::SocketBindError(name, endpoint, err));
            }
        } else {
            trace!("Connecting to ZeroMQ '{}' socket at {}", name, endpoint);
            if let Err(err) = socket.connect(&endpoint) {
                return Err(Error::SocketConnectError(name, endpoint, err));
            }
        }

        Ok(Self {
            socket,
            session,
            name,
        })
    }

    fn new_raw(
        ctx: zmq::Context,
        name: String,
        kind: zmq::SocketType,
        identity: Option<&[u8]>,
    ) -> Result<zmq::Socket, Error> {
        // Create the underlying ZeroMQ socket
        let socket = match ctx.socket(kind) {
            Ok(s) => s,
            Err(err) => return Err(Error::CreateSocketFailed(name, err)),
        };

        // For IOPub in particular, which is fairly high traffic, we up the
        // "high water mark" from the default of 1k -> 100k to avoid dropping
        // messages if the subscriber is processing them too slowly. This has
        // to be set before the call to `bind()`. It seems like we could
        // alternatively set the rcvhwm on the subscriber side, since the
        // "total" sndhmw seems to be the sum of the pub + sub values, but this
        // is probably best to tell any subscribers out there that this is a
        // high traffic channel.
        // https://github.com/posit-dev/amalthea/pull/129
        if name == "IOPub" {
            if let Err(error) = socket.set_sndhwm(100000) {
                return Err(Error::CreateSocketFailed(name, error));
            }
        }

        // Set the socket's identity, if supplied
        if let Some(identity) = identity {
            if let Err(err) = socket.set_identity(identity) {
                return Err(Error::CreateSocketFailed(name, err));
            }
        }

        Ok(socket)
    }

    /// Receive a message from the socket.
    ///
    /// **Note**: This will block until a message is delivered on the socket.
    pub fn recv(&self, msg: &mut zmq::Message) -> Result<(), Error> {
        if let Err(err) = self.socket.recv(msg, 0) {
            Err(Error::ZmqError(self.name.clone(), err))
        } else {
            Ok(())
        }
    }

    /// Receive a multi-part message from the socket.
    ///
    /// **Note**: This will block until a message is delivered on the socket.
    pub fn recv_multipart(&self) -> Result<Vec<Vec<u8>>, Error> {
        match self.socket.recv_multipart(0) {
            Ok(data) => Ok(data),
            Err(err) => Err(Error::ZmqError(self.name.clone(), err)),
        }
    }

    /// Send a message on the socket.
    pub fn send(&self, msg: zmq::Message) -> Result<(), Error> {
        match self.socket.send(msg, 0) {
            Ok(data) => Ok(data),
            Err(err) => Err(Error::ZmqError(self.name.clone(), err)),
        }
    }

    /// Send a multi-part message on the socket.
    pub fn send_multipart(&self, data: &Vec<Vec<u8>>) -> Result<(), Error> {
        match self.socket.send_multipart(data, 0) {
            Ok(data) => Ok(data),
            Err(err) => Err(Error::ZmqError(self.name.clone(), err)),
        }
    }

    pub fn poll_incoming(&self, timeout_ms: i64) -> zmq::Result<bool> {
        Ok(self.socket.poll(zmq::PollEvents::POLLIN, timeout_ms)? != 0)
    }

    pub fn has_incoming_data(&self) -> zmq::Result<bool> {
        self.poll_incoming(0)
    }

    /// Subscribes a SUB socket to all the published messages from a PUB socket.
    ///
    /// Note that this needs to be called *after* the socket connection is
    /// established on both ends.
    pub fn subscribe(&self) -> Result<(), Error> {
        // Currently, all SUB sockets subscribe to all topics; in theory
        // frontends could subscribe selectively, but in practice all known
        // Jupyter frontends subscribe to all topics.
        match self.socket.set_subscribe(b"") {
            Ok(_) => Ok(()),
            Err(err) => Err(Error::ZmqError(self.name.clone(), err)),
        }
    }
}
