/*
 * wire_message.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::header::JupyterHeader;
use generic_array::GenericArray;
use hmac::Hmac;
use serde::{Deserialize, Serialize};
use serde_json::value::Value;
use sha2::Sha256;
use std::fmt;

/// This delimiter separates the ZeroMQ socket identities (IDS) from the message
/// body payload (MSG).
const MSG_DELIM: &[u8] = b"<IDS|MSG>";

/// Represents a Jupyter message
#[derive(Serialize, Deserialize)]
pub struct WireMessage {
    /// The header for this message
    pub header: JupyterHeader,

    /// The header of the message from which this message originated
    pub parent_header: JupyterHeader,

    /// Additional metadata, if any
    pub metadata: Value,

    /// The body (payload) of the message
    pub content: Value,

    /// Additional binary data
    pub buffers: Value,
}

#[derive(Debug)]
pub enum MessageError {
    MissingDelimiter,
    InvalidHmac(Vec<u8>, hex::FromHexError),
    BadSignature(Vec<u8>, hmac::digest::MacError),
}

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MessageError::MissingDelimiter => {
                write!(
                    f,
                    "ZeroMQ message did not include expected <IDS|MSG> delimiter"
                )
            }
            MessageError::InvalidHmac(data, err) => {
                write!(
                    f,
                    "ZeroMQ message HMAC signature {:?} is not a valid hexadecimal value: {}",
                    data, err
                )
            }
            MessageError::BadSignature(sig, err) => {
                write!(
                    f,
                    "ZeroMQ message HMAC signature {:?} is incorrect: {}",
                    sig, err
                )
            }
        }
    }
}

impl WireMessage {
    /// Parse a Jupyter message from an array of buffers (from a ZeroMQ message)
    pub fn from_buffers(
        bufs: Vec<Vec<u8>>,
        hmac_key: Option<Hmac<Sha256>>,
    ) -> Result<WireMessage, MessageError> {
        let mut iter = bufs.iter();

        // Find the position of the <IDS|MSG> delimiter in the message, which
        // separates the socket identities (IDS) from the body of the message.
        let pos = match iter.position(|buf| &buf[..] == MSG_DELIM) {
            Some(p) => p,
            None => return Err(MessageError::MissingDelimiter),
        };
        if let Err(err) = WireMessage::validate_hmac(bufs, hmac_key) {
            return Err(err);
        }

        Err(MessageError::MissingDelimiter)
    }

    fn validate_hmac(
        mut bufs: Vec<Vec<u8>>,
        hmac_key: Option<Hmac<Sha256>>,
    ) -> Result<(), MessageError> {
        use hmac::Mac;

        // If we don't have a key at all, no need to validate. It is acceptable
        // (per Jupyter spec) to have an empty connection key, which indicates
        // that no HMAC signatures are to be validated.
        let key = match hmac_key {
            Some(k) => k,
            None => return Ok(()),
        };

        // TODO: don't unwrap, and this is not actually the hmac
        let data = bufs.pop().unwrap();

        // Decode the hexadecimal representation of the signature
        let decoded = match hex::decode(&data) {
            Ok(decoded_bytes) => decoded_bytes,
            Err(error) => return Err(MessageError::InvalidHmac(data, error)),
        };

        // Compute the real signature according to our own key
        let mut hmac_validator = key.clone();
        for buf in bufs {
            hmac_validator.update(&buf);
        }
        // Verify the signature
        if let Err(err) = hmac_validator.verify(GenericArray::from_slice(&decoded)) {
            return Err(MessageError::BadSignature(decoded, err));
        }

        // If we got this far, the signature is valid
        Ok(())
    }
}
