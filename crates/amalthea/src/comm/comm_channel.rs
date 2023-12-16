/*
 * comm_channel.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use strum_macros::EnumString;
use uuid::Uuid;

use super::frontend_comm::FrontendFrontendRpcRequest;
use crate::wire::jupyter_message::MessageType;

#[derive(EnumString, PartialEq)]
#[strum(serialize_all = "camelCase")]
pub enum Comm {
    /// A variables pane.
    Variables,

    /// A wrapper for a Language Server Protocol server.
    Lsp,

    /// A wrapper for a Debug Adapter Protocol server.
    Dap,

    /// A dynamic (resizable) plot.
    Plot,

    /// A data viewer.
    DataViewer,

    /// The Positron help pane.
    Help,

    /// The Positron front end.
    FrontEnd,

    /// Some other comm with a custom name.
    Other(String),
}

// TODO: Rename to Request and Reply?
#[derive(Clone, Debug)]
pub enum CommMsg {
    /// A message that is part of a Remote Procedure Call (RPC). The first value
    /// is the unique ID of the RPC invocation (i.e. the Jupyter message ID),
    /// and the second value is the data associated with the RPC (the request or
    /// response).
    Rpc(String, Value),

    /// A message representing any other data sent on the comm channel; usually
    /// used for events.
    Data(Value),

    // A message indicating that the comm channel should be closed.
    Close,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct RpcRequest {
    msg_type: String,
    id: String,
    request: Value,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct RpcRequestRaw {
    pub method: String,
    pub params: Vec<Value>,
}

impl RpcRequest {
    pub fn new(request: FrontendFrontendRpcRequest) -> anyhow::Result<Self> {
        let request = Self {
            msg_type: String::from("rpc_request"),
            id: Uuid::new_v4().to_string(),
            request: serde_json::to_value(request)?,
        };
        Ok(request)
    }

    pub fn from_raw(request: RpcRequestRaw) -> anyhow::Result<Self> {
        let request = Self {
            msg_type: String::from("rpc_request"),
            id: Uuid::new_v4().to_string(),
            request: serde_json::to_value(request)?,
        };
        Ok(request)
    }
}

impl MessageType for RpcRequest {
    fn message_type() -> String {
        String::from("rpc_request")
    }
}
