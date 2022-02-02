/*
 * signed_socket.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use hmac::Hmac;
use sha2::Sha256;

/// Represents a socket that sends and receives messages that are optionally
/// signed with a SHA-256 HMAC.
pub struct SignedSocket {
    /// A ZeroMQ socket over which signed messages are to be sent/received
    pub socket: zmq::Socket,

    /// The HMAC SHA-256 key, or None if the connection is to be unauthenticated
    pub hmac: Option<Hmac<Sha256>>,
}
