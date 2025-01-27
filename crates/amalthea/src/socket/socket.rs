/*
 * socket.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

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

        // For the server side of IOPub, there are a few options we need to tweak
        if name == "IOPub" && kind == zmq::SocketType::XPUB {
            // Sets the XPUB socket to report subscription events even for
            // topics that were already subscribed to.
            //
            // See notes in https://zguide.zeromq.org/docs/chapter5 and
            // https://zguide.zeromq.org/docs/chapter6 and the discussion in
            // https://lists.zeromq.org/pipermail/zeromq-dev/2012-October/018470.html
            // that lead to the creation of this socket option.
            socket
                .set_xpub_verbose(true)
                .map_err(|err| Error::CreateSocketFailed(name.clone(), err))?;

            // For IOPub in particular, which is fairly high traffic, we up the
            // "high water mark" from the default of 1k -> 100k to avoid dropping
            // messages if the subscriber is processing them too slowly. This has
            // to be set before the call to `bind()`. It seems like we could
            // alternatively set the rcvhwm on the subscriber side, since the
            // "total" sndhmw seems to be the sum of the pub + sub values, but this
            // is probably best to tell any subscribers out there that this is a
            // high traffic channel.
            // https://github.com/posit-dev/amalthea/pull/129
            socket
                .set_sndhwm(100000)
                .map_err(|err| Error::CreateSocketFailed(name.clone(), err))?;
        }

        if name == "IOPub" && kind == zmq::SocketType::SUB {
            // For the client side of IOPub (in tests and eventually kallichore), we need
            // to subscribe our SUB to messages from the XPUB on the server side. We use
            // `""` to subscribe to all message types, there is no reason to filter any
            // out. It is very important that we subscribe BEFORE we `connect()`. If we
            // don't subscribe first, then the XPUB on the server side can come online
            // first and processes our `connect()` before we've actually subscribed, which
            // causes the welcome message the XPUB sends us to get dropped, preventing us
            // from correctly starting up, because we block until we've received that
            // welcome message. In the link below, you can see proof that zmq only sends
            // the welcome message out when it processes our `connect()` call, so if we
            // aren't subscribed by that point, we miss it.
            // https://github.com/zeromq/libzmq/blob/34f7fa22022bed9e0e390ed3580a1c83ac4a2834/src/xpub.cpp#L56-L65
            socket
                .set_subscribe(b"")
                .map_err(|err| Error::CreateSocketFailed(name.clone(), err))?;
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

        // One side of a socket must `bind()` to its endpoint, and the other
        // side must `connect()` to the same endpoint. The `bind()` side
        // will be the server, and the `connect()` side will be the client.
        match kind {
            zmq::SocketType::ROUTER | zmq::SocketType::XPUB | zmq::SocketType::REP => {
                log::trace!("Binding to ZeroMQ '{}' socket at {}", name, endpoint);
                if let Err(err) = socket.bind(&endpoint) {
                    return Err(Error::SocketBindError(name, endpoint, err));
                }
            },
            zmq::SocketType::DEALER | zmq::SocketType::SUB | zmq::SocketType::REQ => {
                log::trace!("Connecting to ZeroMQ '{}' socket at {}", name, endpoint);
                if let Err(err) = socket.connect(&endpoint) {
                    return Err(Error::SocketConnectError(name, endpoint, err));
                }
            },
            _ => return Err(Error::UnsupportedSocketType(kind)),
        }

        // Create a new socket and return
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
            log::trace!("Binding to ZeroMQ '{}' socket at {}", name, endpoint);
            if let Err(err) = socket.bind(&endpoint) {
                return Err(Error::SocketBindError(name, endpoint, err));
            }
        } else {
            log::trace!("Connecting to ZeroMQ '{}' socket at {}", name, endpoint);
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
}
