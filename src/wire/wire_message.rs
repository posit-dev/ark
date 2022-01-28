/*
 * wire_message.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::header::JupyterHeader;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::MessageType;
use generic_array::GenericArray;
use hmac::Hmac;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::value::Value;
use sha2::Sha256;
use std::fmt;

/// This delimiter separates the ZeroMQ socket identities (IDS) from the message
/// body payload (MSG).
const MSG_DELIM: &[u8] = b"<IDS|MSG>";

/// Represents an untyped Jupyter message delivered over the wire.
#[derive(Serialize, Deserialize)]
pub struct WireMessage {
    /// The header for this message
    pub header: JupyterHeader,

    /// The header of the message from which this message originated, if any
    pub parent_header: Option<JupyterHeader>,

    /// Additional metadata, if any
    pub metadata: Value,

    /// The body (payload) of the message
    pub content: Value,
}

#[derive(Debug)]
pub enum MessageError {
    SocketRead(zmq::Error),
    MissingDelimiter,
    InsufficientParts(usize, usize),
    InvalidHmac(Vec<u8>, hex::FromHexError),
    BadSignature(Vec<u8>, hmac::digest::MacError),
    Utf8Error(String, Vec<u8>, std::str::Utf8Error),
    JsonParseError(String, String, serde_json::Error),
    InvalidPart(String, serde_json::Value, serde_json::Error),
    InvalidMessage(String, serde_json::Value, serde_json::Error),
    CannotSerialize(serde_json::Error),
    CannotSend(zmq::Error),
    UnknownType(String),
}

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            MessageError::SocketRead(err) => {
                write!(f, "Could not read ZeroMQ message from socket: {}", err)
            }
            MessageError::MissingDelimiter => {
                write!(
                    f,
                    "ZeroMQ message did not include expected <IDS|MSG> delimiter"
                )
            }
            MessageError::InsufficientParts(found, expected) => {
                write!(
                    f,
                    "ZeroMQ message did not contain sufficient parts (found {}, expected {})",
                    found, expected
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
            MessageError::Utf8Error(part, data, err) => {
                write!(
                    f,
                    "Message part '{}' was not valid UTF-8: {} (raw: {:?})",
                    part, err, data
                )
            }
            MessageError::JsonParseError(part, str, err) => {
                write!(
                    f,
                    "Message part '{}' is invalid JSON: {} (raw: {})",
                    part, err, str
                )
            }
            MessageError::InvalidPart(part, json, err) => {
                write!(
                    f,
                    "Message part '{}' does not match schema: {} (raw: {})",
                    part, err, json
                )
            }
            MessageError::InvalidMessage(kind, json, err) => {
                write!(f, "Invalid '{}' message: {} (raw: {})", kind, err, json)
            }
            MessageError::UnknownType(kind) => {
                write!(f, "Unknown message type '{}'", kind)
            }
            MessageError::CannotSerialize(err) => {
                write!(f, "Cannot serialize message: {}", err)
            }
            MessageError::CannotSend(err) => {
                write!(f, "Cannot send message: {}", err)
            }
        }
    }
}

impl WireMessage {
    pub fn read_from_socket(
        socket: &zmq::Socket,
        hmac_key: Option<Hmac<Sha256>>,
    ) -> Result<WireMessage, MessageError> {
        match socket.recv_multipart(0) {
            Ok(bufs) => Self::from_buffers(bufs, hmac_key),
            Err(err) => Err(MessageError::SocketRead(err)),
        }
    }
    /// Parse a Jupyter message from an array of buffers (from a ZeroMQ message)
    pub fn from_buffers(
        mut bufs: Vec<Vec<u8>>,
        hmac_key: Option<Hmac<Sha256>>,
    ) -> Result<WireMessage, MessageError> {
        let mut iter = bufs.iter();

        // Find the position of the <IDS|MSG> delimiter in the message, which
        // separates the socket identities (IDS) from the body of the message
        // (MSG).
        let pos = match iter.position(|buf| &buf[..] == MSG_DELIM) {
            Some(p) => p,
            None => return Err(MessageError::MissingDelimiter),
        };

        // Form a collection of the remaining parts.
        let parts: Vec<_> = bufs.drain(pos + 1..).collect();

        // We expect to have at least 5 parts left (the HMAC + 4 message frames)
        if parts.len() < 4 {
            return Err(MessageError::InsufficientParts(parts.len(), 4));
        }

        // Consume and validate the HMAC signature.
        WireMessage::validate_hmac(&parts, hmac_key)?;

        // Parse the message header
        let header_val = WireMessage::parse_buffer(String::from("header"), &parts[1])?;
        let header: JupyterHeader = match serde_json::from_value(header_val.clone()) {
            Ok(h) => h,
            Err(err) => {
                return Err(MessageError::InvalidPart(
                    String::from("header"),
                    header_val,
                    err,
                ))
            }
        };

        // Parse the parent header.
        let parent: Option<JupyterHeader> = match parts[2].len() {
            0 | 1 | 2 => {
                // If there is no meaningful content in the parent header
                // buffer, we have no parent message, which is OK per the wire
                // protocol.
                None
            }
            _ => {
                // If we do have content, ensure it parses as a header.
                let parent_val =
                    WireMessage::parse_buffer(String::from("parent header"), &parts[2])?;
                match serde_json::from_value(parent_val.clone()) {
                    Ok(h) => Some(h),
                    Err(err) => {
                        return Err(MessageError::InvalidPart(
                            String::from("parent header"),
                            parent_val,
                            err,
                        ))
                    }
                }
            }
        };

        Ok(Self {
            header: header,
            parent_header: parent,
            metadata: WireMessage::parse_buffer(String::from("metadata"), &parts[2])?,
            content: WireMessage::parse_buffer(String::from("content"), &parts[3])?,
        })
    }

    /// Validates the message's HMAC signature
    fn validate_hmac(
        bufs: &Vec<Vec<u8>>,
        hmac_key: Option<Hmac<Sha256>>,
    ) -> Result<(), MessageError> {
        use hmac::Mac;

        // The hmac signature is the first value
        let data = &bufs[0];

        // If we don't have a key at all, no need to validate. It is acceptable
        // (per Jupyter spec) to have an empty connection key, which indicates
        // that no HMAC signatures are to be validated.
        let key = match hmac_key {
            Some(k) => k,
            None => return Ok(()),
        };

        // Decode the hexadecimal representation of the signature
        let decoded = match hex::decode(&data) {
            Ok(decoded_bytes) => decoded_bytes,
            Err(error) => return Err(MessageError::InvalidHmac(data.to_vec(), error)),
        };

        // Compute the real signature according to our own key
        let mut hmac_validator = key.clone();
        let mut key = true;
        for buf in bufs {
            // Skip the key itself when computing checksum
            if key {
                key = false;
                continue;
            }
            hmac_validator.update(&buf);
        }
        // Verify the signature
        if let Err(err) = hmac_validator.verify(GenericArray::from_slice(&decoded)) {
            return Err(MessageError::BadSignature(decoded, err));
        }

        // Signature is valid
        Ok(())
    }

    fn parse_buffer(desc: String, buf: &[u8]) -> Result<serde_json::Value, MessageError> {
        // Convert the raw byte sequence from the ZeroMQ message into UTF-8
        let str = match std::str::from_utf8(&buf) {
            Ok(s) => s,
            Err(err) => return Err(MessageError::Utf8Error(desc, buf.to_vec(), err)),
        };

        // Parse the UTF-8 string as JSON
        let val: serde_json::Value = match serde_json::from_str(str) {
            Ok(v) => v,
            Err(err) => return Err(MessageError::JsonParseError(desc, String::from(str), err)),
        };

        Ok(val)
    }

    pub fn send(
        &self,
        socket: &zmq::Socket,
        hmac_key: Option<Hmac<Sha256>>,
    ) -> Result<(), MessageError> {
        // Serialize JSON values into byte parts in preparation for transmission
        let mut parts: Vec<Vec<u8>> = match self.to_raw_parts() {
            Ok(v) => v,
            Err(err) => return Err(MessageError::CannotSerialize(err)),
        };

        // Compute HMAC signature
        let hmac = match hmac_key {
            Some(key) => {
                use hmac::Mac;
                let mut sig = key.clone();
                for part in &parts {
                    sig.update(&part);
                }
                hex::encode(sig.finalize().into_bytes().as_slice())
            }
            None => String::new(),
        };

        // Create vector to store message to be delivered
        let mut msg: Vec<Vec<u8>> = Vec::new();

        // TODO: Add ZeroMQ socket identities here!

        // Add <IDS|MSG> delimiter
        msg.push(MSG_DELIM.to_vec());

        // Add HMAC signature
        msg.push(hmac.as_bytes().to_vec());

        // Add all the message parts
        msg.append(&mut parts);

        // Deliver the message!
        if let Err(err) = socket.send_multipart(&msg, 0) {
            return Err(MessageError::CannotSend(err));
        }

        // Successful delivery
        Ok(())
    }

    /// Returns a vector containing the raw parts of the message
    fn to_raw_parts(&self) -> Result<Vec<Vec<u8>>, serde_json::Error> {
        let mut parts: Vec<Vec<u8>> = Vec::new();
        parts.push(serde_json::to_vec(&self.header)?);
        parts.push(serde_json::to_vec(&self.parent_header)?);
        parts.push(serde_json::to_vec(&self.metadata)?);
        parts.push(serde_json::to_vec(&self.content)?);
        Ok(parts)
    }

    pub fn from_jupyter_message<T>(msg: JupyterMessage<T>) -> Result<Self, MessageError>
    where
        T: MessageType + Serialize,
    {
        let content = match serde_json::to_value(msg.content) {
            Ok(val) => val,
            Err(err) => return Err(MessageError::CannotSerialize(err)),
        };
        Ok(Self {
            header: msg.header,
            parent_header: msg.parent_header,
            metadata: json!({}),
            content: content,
        })
    }

    /// Converts this wire message to a Jupyter message of type T
    pub fn to_message_type<T>(&self) -> Result<JupyterMessage<T>, MessageError>
    where
        T: MessageType + DeserializeOwned,
    {
        let content = match serde_json::from_value(self.content.clone()) {
            Ok(val) => val,
            Err(err) => {
                return Err(MessageError::InvalidMessage(
                    T::message_type(),
                    self.content.clone(),
                    err,
                ))
            }
        };
        Ok(JupyterMessage {
            header: self.header.clone(),
            parent_header: self.parent_header.clone(),
            content: content,
        })
    }
}
