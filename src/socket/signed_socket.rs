/*
 * signed_socket.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Represents a socket that sends and receives messages that are optionally
/// signed with a SHA-256 HMAC.
pub struct SignedSocket {
    /// A ZeroMQ socket over which signed messages are to be sent/received
    socket: zmq::Socket,

    /// The HMAC SHA-256 key, or None if the connection is to be unauthenticated
    hmac: Option<Hmac<Sha256>>,
}

impl SignedSocket {
    pub fn new(socket: zmq::Socket, hmac_key: &str) -> Result<SignedSocket, Error> {
        let key = match hmac_key.len() {
            0 => None,
            _ => {
                let result = match Hmac::<Sha256>::new_from_slice(hmac_key.as_bytes()) {
                    Ok(hmac) => hmac,
                    Err(err) => return Err(Error::HmacKeyInvalid(hmac_key.to_string(), err)),
                };
                Some(result)
            }
        };
        Ok(SignedSocket {
            socket: socket,
            hmac: key,
        })
    }
}
