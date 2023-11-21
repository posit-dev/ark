/*
 * frontend_comm.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;

use crate::wire::client_event::ClientEvent;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum FrontendMessage {
    Event(ClientEvent),
    RpcRequest(FrontendRpcRequest),
    RpcResultResponse(FrontendRpcResult),
    RpcResultError(FrontendRpcError),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum FrontendRpcRequest {
    Method(String),
    Params(Vec<Value>),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum FrontendRpcResult {
    Result(Value),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum FrontendRpcErrorData {
    Message(String),
    Code(i32),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum FrontendRpcError {
    Error(FrontendRpcErrorData),
}
