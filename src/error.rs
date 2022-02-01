/*
 * error.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use std::fmt;

#[derive(Debug)]
pub enum Error {
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
    UnknownMessageType(String),
    NoInstallDir,
    CreateDirFailed(std::io::Error),
    JsonSerializeSpecFailed(serde_json::Error),
    CreateSpecFailed(std::io::Error),
    WriteSpecFailed(std::io::Error),
    HmacKeyInvalid(String, crypto_common::InvalidLength),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::SocketRead(err) => {
                write!(f, "Could not read ZeroMQ message from socket: {}", err)
            }
            Error::MissingDelimiter => {
                write!(
                    f,
                    "ZeroMQ message did not include expected <IDS|MSG> delimiter"
                )
            }
            Error::InsufficientParts(found, expected) => {
                write!(
                    f,
                    "ZeroMQ message did not contain sufficient parts (found {}, expected {})",
                    found, expected
                )
            }
            Error::InvalidHmac(data, err) => {
                write!(
                    f,
                    "ZeroMQ message HMAC signature {:?} is not a valid hexadecimal value: {}",
                    data, err
                )
            }
            Error::BadSignature(sig, err) => {
                write!(
                    f,
                    "ZeroMQ message HMAC signature {:?} is incorrect: {}",
                    sig, err
                )
            }
            Error::Utf8Error(part, data, err) => {
                write!(
                    f,
                    "Message part '{}' was not valid UTF-8: {} (raw: {:?})",
                    part, err, data
                )
            }
            Error::JsonParseError(part, str, err) => {
                write!(
                    f,
                    "Message part '{}' is invalid JSON: {} (raw: {})",
                    part, err, str
                )
            }
            Error::InvalidPart(part, json, err) => {
                write!(
                    f,
                    "Message part '{}' does not match schema: {} (raw: {})",
                    part, err, json
                )
            }
            Error::InvalidMessage(kind, json, err) => {
                write!(f, "Invalid '{}' message: {} (raw: {})", kind, err, json)
            }
            Error::UnknownMessageType(kind) => {
                write!(f, "Unknown message type '{}'", kind)
            }
            Error::CannotSerialize(err) => {
                write!(f, "Cannot serialize message: {}", err)
            }
            Error::CannotSend(err) => {
                write!(f, "Cannot send message: {}", err)
            }
            Error::NoInstallDir => {
                write!(f, "No Jupyter installation directory found.")
            }
            Error::CreateDirFailed(err) => {
                write!(f, "Could not create directory: {}", err)
            }
            Error::JsonSerializeSpecFailed(err) => {
                write!(f, "Could not serialize kernel spec to JSON: {}", err)
            }
            Error::CreateSpecFailed(err) => {
                write!(f, "Could not create kernel spec file: {}", err)
            }
            Error::WriteSpecFailed(err) => {
                write!(f, "Could not write kernel spec file: {}", err)
            }
            Error::HmacKeyInvalid(str, err) => {
                write!(
                    f,
                    "The HMAC supplied signing key '{}' ({} bytes) cannot be used: {}",
                    str,
                    str.len(),
                    err
                )
            }
        }
    }
}
