/*
 * base_comm.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;
use serde_json::json;
use serde_json::Value;
use serde_repr::Deserialize_repr;
use serde_repr::Serialize_repr;

use crate::wire::jupyter_message::MessageType;

// TODO: Do we require `id` fields for comm replies over StdIn?
// Or can we use the Jupyter parent header easily instead?
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonRpcResponse {
    Result { id: String, result: Value },
    Error { id: String, error: JsonRpcErrorData },
}

impl MessageType for JsonRpcResponse {
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
