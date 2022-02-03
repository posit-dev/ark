/*
 * wire_message.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::socket::signed_socket::SignedSocket;
use crate::wire::header::JupyterHeader;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::MessageType;
use crate::wire::jupyter_message::ProtocolMessage;
use generic_array::GenericArray;
use hmac::Hmac;
use log::trace;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::value::Value;
use sha2::Sha256;

/// This delimiter separates the ZeroMQ socket identities (IDS) from the message
/// body payload (MSG).
const MSG_DELIM: &[u8] = b"<IDS|MSG>";

/// Represents an untyped Jupyter message delivered over the wire. A WireMessage
/// can represent any kind of Jupyter message; typically its header will be
/// examined and it will be converted into a typed JupyterMessage.
#[derive(Serialize, Deserialize)]
pub struct WireMessage {
    /// The ZeroMQ identities. These store the peer identity for messages
    /// delivered request-reply style over ROUTER sockets (like the shell)
    pub zmq_identities: Vec<Vec<u8>>,

    /// The header for this message
    pub header: JupyterHeader,

    /// The header of the message from which this message originated, if any
    pub parent_header: Option<JupyterHeader>,

    /// Additional metadata, if any
    pub metadata: Value,

    /// The body (payload) of the message
    pub content: Value,
}

impl WireMessage {
    pub fn read_from_socket(socket: &SignedSocket) -> Result<WireMessage, Error> {
        match socket.socket.recv_multipart(0) {
            Ok(bufs) => Self::from_buffers(bufs, &socket.hmac),
            Err(err) => Err(Error::SocketRead(err)),
        }
    }

    /// Parse a Jupyter message from an array of buffers (from a ZeroMQ message)
    pub fn from_buffers(
        mut bufs: Vec<Vec<u8>>,
        hmac_key: &Option<Hmac<Sha256>>,
    ) -> Result<WireMessage, Error> {
        let mut iter = bufs.iter();

        // Find the position of the <IDS|MSG> delimiter in the message, which
        // separates the socket identities (IDS) from the body of the message
        // (MSG).
        let pos = match iter.position(|buf| &buf[..] == MSG_DELIM) {
            Some(p) => p,
            None => return Err(Error::MissingDelimiter),
        };

        // Form a collection of the remaining parts, and remove the delimiter.
        let parts: Vec<_> = bufs.drain(pos + 1..).collect();
        bufs.pop();

        // We expect to have at least 5 parts left (the HMAC + 4 message frames)
        if parts.len() < 4 {
            return Err(Error::InsufficientParts(parts.len(), 4));
        }

        // Consume and validate the HMAC signature.
        WireMessage::validate_hmac(&parts, hmac_key)?;

        // Parse the message header
        let header_val = WireMessage::parse_buffer(String::from("header"), &parts[1])?;
        let header: JupyterHeader = match serde_json::from_value(header_val.clone()) {
            Ok(h) => h,
            Err(err) => return Err(Error::InvalidPart(String::from("header"), header_val, err)),
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
                        return Err(Error::InvalidPart(
                            String::from("parent header"),
                            parent_val,
                            err,
                        ))
                    }
                }
            }
        };

        Ok(Self {
            zmq_identities: bufs,
            header: header,
            parent_header: parent,
            metadata: WireMessage::parse_buffer(String::from("metadata"), &parts[3])?,
            content: WireMessage::parse_buffer(String::from("content"), &parts[4])?,
        })
    }

    /// Validates the message's HMAC signature
    fn validate_hmac(bufs: &Vec<Vec<u8>>, hmac_key: &Option<Hmac<Sha256>>) -> Result<(), Error> {
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
            Err(error) => return Err(Error::InvalidHmac(data.to_vec(), error)),
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
            return Err(Error::BadSignature(decoded, err));
        }

        // Signature is valid
        Ok(())
    }

    fn parse_buffer(desc: String, buf: &[u8]) -> Result<serde_json::Value, Error> {
        // Convert the raw byte sequence from the ZeroMQ message into UTF-8
        let str = match std::str::from_utf8(&buf) {
            Ok(s) => s,
            Err(err) => return Err(Error::Utf8Error(desc, buf.to_vec(), err)),
        };

        // Parse the UTF-8 string as JSON
        let val: serde_json::Value = match serde_json::from_str(str) {
            Ok(v) => v,
            Err(err) => return Err(Error::JsonParseError(desc, String::from(str), err)),
        };

        Ok(val)
    }

    pub fn send(&self, socket: &SignedSocket) -> Result<(), Error> {
        // Serialize JSON values into byte parts in preparation for transmission
        let mut parts: Vec<Vec<u8>> = match self.to_raw_parts() {
            Ok(v) => v,
            Err(err) => return Err(Error::CannotSerialize(err)),
        };

        // Compute HMAC signature
        let hmac = match &socket.hmac {
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

        // Create vector to store message to be delivered; start with the socket identities, if any
        let mut msg: Vec<Vec<u8>> = self.zmq_identities.clone();

        // Add <IDS|MSG> delimiter
        msg.push(MSG_DELIM.to_vec());

        // Add HMAC signature
        msg.push(hmac.as_bytes().to_vec());

        // Add all the message parts
        msg.append(&mut parts);

        // Deliver the message!
        if let Err(err) = socket.socket.send_multipart(&msg, 0) {
            return Err(Error::CannotSend(err));
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

    pub fn from_jupyter_message<T>(msg: JupyterMessage<T>) -> Result<Self, Error>
    where
        T: ProtocolMessage,
    {
        let content = match serde_json::to_value(msg.content) {
            Ok(val) => val,
            Err(err) => return Err(Error::CannotSerialize(err)),
        };
        Ok(Self {
            zmq_identities: msg.zmq_identities.clone(),
            header: msg.header,
            parent_header: msg.parent_header,
            metadata: json!({}),
            content: content,
        })
    }

    /// Converts this wire message to a Jupyter message of type T
    pub fn to_message_type<T>(&self) -> Result<JupyterMessage<T>, Error>
    where
        T: MessageType + DeserializeOwned,
    {
        let content = match serde_json::from_value(self.content.clone()) {
            Ok(val) => val,
            Err(err) => {
                return Err(Error::InvalidMessage(
                    T::message_type(),
                    self.content.clone(),
                    err,
                ))
            }
        };
        Ok(JupyterMessage {
            zmq_identities: self.zmq_identities.clone(),
            header: self.header.clone(),
            parent_header: self.parent_header.clone(),
            content: content,
        })
    }
}
