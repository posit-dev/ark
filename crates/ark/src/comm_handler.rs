//
// comm_handler.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::fmt::Debug;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use amalthea::comm::base_comm::json_rpc_error;
use amalthea::comm::base_comm::JsonRpcErrorCode;
use amalthea::comm::comm_channel::CommMsg;
use amalthea::socket::comm::CommOutgoingTx;
use serde::de::DeserializeOwned;
use serde::Serialize;
use stdext::result::ResultExt;

/// Context provided to `CommHandler` methods, giving access to the outgoing
/// channel and close-request mechanism. In the future, we'll provide access to
/// more of the Console state, such as the currently active environment.
#[derive(Debug)]
pub struct CommHandlerContext {
    pub outgoing_tx: CommOutgoingTx,
    closed: AtomicBool,
}

impl CommHandlerContext {
    pub fn new(outgoing_tx: CommOutgoingTx) -> Self {
        Self {
            outgoing_tx,
            closed: AtomicBool::new(false),
        }
    }

    /// Request that Console close this comm after the current handler method
    /// returns. The handler can still send responses or events before and
    /// after calling this since cleanup is deferred.
    pub fn close_on_exit(&self) {
        self.closed.store(true, Ordering::Relaxed);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }
}

/// Trait for comm handlers that run synchronously on the R thread.
///
/// All methods are called from the R thread within `ReadConsole`, so R code
/// can be called directly without `r_task()`.
pub trait CommHandler: Send + Debug {
    /// Initialise handler state on the R thread (initial scan, first event,
    /// etc.). Default is no-op.
    fn handle_open(&mut self, _ctx: &CommHandlerContext) {}

    /// Handle an incoming message (RPC or data).
    fn handle_msg(&mut self, msg: CommMsg, ctx: &CommHandlerContext);

    /// Handle comm close. Default is no-op.
    fn handle_close(&mut self, _ctx: &CommHandlerContext) {}

    /// Called when the environment changes (e.g. after top-level execution,
    /// entering/exiting debug, or when frame is selected in call stack).
    /// Default is no-op.
    fn handle_environment(&mut self, _ctx: &CommHandlerContext) {}
}

/// A registered comm in the Console's comm table.
pub struct RegisteredComm {
    pub handler: Box<dyn CommHandler>,
    pub ctx: CommHandlerContext,
    pub comm_name: String,
}

/// Handle an RPC request from a `CommMsg`. This is the blocking-comm equivalent
/// of `CommSocket::handle_request`.
///
/// Non-RPC messages are logged and ignored. Requests that could not be
/// handled cause an RPC error response.
pub fn handle_rpc_request<Reqs, Reps>(
    outgoing_tx: &CommOutgoingTx,
    comm_name: &str,
    message: CommMsg,
    request_handler: impl FnOnce(Reqs) -> anyhow::Result<Reps>,
) where
    Reqs: DeserializeOwned + Debug,
    Reps: Serialize,
{
    let (id, parent_header, data) = match message {
        CommMsg::Rpc {
            id,
            parent_header,
            data,
        } => (id, parent_header, data),
        other => {
            log::warn!("Expected RPC message for {comm_name}, got {other:?}");
            return;
        },
    };

    let json = match serde_json::from_value::<Reqs>(data.clone()) {
        Ok(m) => {
            let _span =
                tracing::trace_span!("comm handler", name = comm_name, request = ?m).entered();
            match request_handler(m) {
                Ok(reply) => match serde_json::to_value(reply) {
                    Ok(value) => value,
                    Err(err) => {
                        let message = format!(
                            "Failed to serialise reply for {comm_name} request: {err} (request: {data})"
                        );
                        log::warn!("{message}");
                        json_rpc_error(JsonRpcErrorCode::InternalError, message)
                    },
                },
                Err(err) => {
                    let message =
                        format!("Failed to process {comm_name} request: {err} (request: {data})");
                    log::warn!("{message}");
                    json_rpc_error(JsonRpcErrorCode::InternalError, message)
                },
            }
        },
        Err(err) => {
            let message = format!(
                "No handler for {comm_name} request (method not found): {err} (request: {data})"
            );
            log::warn!("{message}");
            json_rpc_error(JsonRpcErrorCode::MethodNotFound, message)
        },
    };

    let response = CommMsg::Rpc {
        id,
        parent_header,
        data: json,
    };
    outgoing_tx.send(response).log_err();
}
