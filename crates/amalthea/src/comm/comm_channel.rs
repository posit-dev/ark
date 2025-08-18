/*
 * comm_channel.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde_json::Value;
use strum_macros::EnumString;

use super::ui_comm::UiFrontendRequest;
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

    /// The Positron frontend.
    Ui,

    /// Some other comm with a custom name.
    Other(String),
}

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

impl MessageType for UiFrontendRequest {
    fn message_type() -> String {
        String::from("rpc_request")
    }
}

/// Create an RPC message nested in a comm message
/// Creates a `CommMsg::Data` with `method` and `params` fields.
///
/// Example usage:
///
/// ```
/// comm_rpc_message!("my_method")
/// comm_rpc_message!("my_method", foo = 1, bar = my_value)
/// ```
#[macro_export]
macro_rules! comm_rpc_message {
    ($method:expr) => {
        CommMsg::Data(serde_json::json!({
            "method": $method,
            "params": {}
        }))
    };
    ($method:expr, $($param_key:ident = $param_value:expr),+ $(,)?) => {
        CommMsg::Data(serde_json::json!({
            "method": $method,
            "params": {
                $(
                    stringify!($param_key): $param_value
                ),*
            }
        }))
    };
}

pub fn comm_rpc_message(method: &str, params: Value) -> CommMsg {
    CommMsg::Data(serde_json::json!({
        "method": method,
        "params": params
    }))
}
