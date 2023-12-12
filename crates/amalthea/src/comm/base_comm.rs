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

/// JSON-RPC 2.0 error codes
#[derive(Debug, Serialize, Deserialize, PartialEq)]
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
