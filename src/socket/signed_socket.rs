/*
 * signed_socket.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::session::Session;

/// Represents a socket that sends and receives messages that are optionally
/// signed with a SHA-256 HMAC.
pub struct SignedSocket {
    /// A ZeroMQ socket over which signed messages are to be sent/received
    pub socket: zmq::Socket,

    /// The Jupyter session information associated with the socket, including
    /// the session ID and HMAC signing key
    pub session: Session,
}
