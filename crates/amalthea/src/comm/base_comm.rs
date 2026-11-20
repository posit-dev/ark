/*
 * base_comm.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use std::error::Error;
use std::fmt;

use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use serde_repr::Deserialize_repr;
use serde_repr::Serialize_repr;

use crate::wire::jupyter_message::MessageType;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcReply {
    Result(JsonRpcResult),
    Error(JsonRpcError),
}

impl MessageType for JsonRpcReply {
    fn message_type() -> String {
        String::from("rpc_reply")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JsonRpcResult {
    pub result: Value,
}

/// JSON-RPC 2.0 error codes
#[derive(Copy, Clone, Debug, Serialize_repr, Deserialize_repr, PartialEq)]
#[repr(i64)]
pub enum JsonRpcErrorCode {
    ParseError = -32700,
    InvalidRequest = -32600,
    MethodNotFound = -32601,
    InvalidParams = -32602,
    InternalError = -32603,
    ServerErrorStart = -32099,
    ServerErrorEnd = -32000,
    /// A plot could not be rendered because the requested size is too small
    /// for it, e.g. its margins don't fit.
    PlotTooSmall = -32001,
}

/// An error that a comm request handler can return to reply with a specific
/// JSON-RPC error code instead of `InternalError`.
#[derive(Debug)]
pub struct RpcError {
    pub code: JsonRpcErrorCode,
    pub message: String,
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl Error for RpcError {}

/// The JSON-RPC error code for a failed request: the code of an `RpcError`
/// anywhere in the error chain, or `InternalError`.
pub fn json_rpc_error_code(err: &anyhow::Error) -> JsonRpcErrorCode {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<RpcError>())
        .map_or(JsonRpcErrorCode::InternalError, |rpc_error| rpc_error.code)
}

/**
 * Create a JSON-RPC 2.0 error response
 *
 * - `code` - The error code
 * - `message` - The error message
 *
 * Returns a JSON object representing the error.
 */
pub fn json_rpc_error(code: JsonRpcErrorCode, message: String) -> Value {
    json! ({
        "error": {
            "code": code,
            "message": message,
            "data": null,
        }
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JsonRpcError {
    pub error: JsonRpcErrorData,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JsonRpcErrorData {
    pub message: String,
    pub code: JsonRpcErrorCode,
}
