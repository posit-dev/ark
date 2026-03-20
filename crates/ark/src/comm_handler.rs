//
// comm_handler.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use std::cell::Cell;
use std::fmt::Debug;

use amalthea::comm::base_comm::json_rpc_error;
use amalthea::comm::base_comm::JsonRpcErrorCode;
use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommEvent;
use amalthea::socket::comm::CommOutgoingTx;
use crossbeam::channel::Sender;
use serde::de::DeserializeOwned;
use serde::Serialize;
use stdext::result::ResultExt;

/// Context provided to `CommHandler` methods, giving access to the outgoing
/// channel and close-request mechanism. In the future, we'll provide access to
/// more of the Console state, such as the currently active environment.
#[derive(Debug)]
pub struct CommHandlerContext {
    pub outgoing_tx: CommOutgoingTx,
    pub comm_event_tx: Sender<CommEvent>,
    closed: Cell<bool>,
}

impl CommHandlerContext {
    pub fn new(outgoing_tx: CommOutgoingTx, comm_event_tx: Sender<CommEvent>) -> Self {
        Self {
            outgoing_tx,
            comm_event_tx,
            closed: Cell::new(false),
        }
    }

    /// Request that Console close this comm after the current handler method
    /// returns. The handler can still send responses or events before and
    /// after calling this since cleanup is deferred.
    pub fn close_on_exit(&self) {
        self.closed.set(true);
    }

    pub fn is_closed(&self) -> bool {
        self.closed.get()
    }

    /// Send a serializable event as `CommMsg::Data` on the outgoing channel.
    /// Serialization or send errors are logged and ignored.
    pub fn send_event<T: Serialize>(&self, event: &T) {
        let Some(json) = serde_json::to_value(event).log_err() else {
            return;
        };
        self.outgoing_tx.send(CommMsg::Data(json)).log_err();
    }
}

/// Trait for comm handlers that run synchronously on the R thread.
///
/// All methods are called from the R thread within `ReadConsole`, so R code
/// can be safely called from these handlers.
pub trait CommHandler: Debug {
    /// Metadata sent to the frontend in the `comm_open` message
    /// (backend-initiated comms). Default is empty object.
    fn open_metadata(&self) -> serde_json::Value {
        serde_json::json!({})
    }

    /// Initialise handler state on the R thread (initial scan, first event,
    /// etc.). Default is no-op.
    fn handle_open(&mut self, _ctx: &CommHandlerContext) {}

    /// Handle an incoming message (RPC or data).
    fn handle_msg(&mut self, msg: CommMsg, ctx: &CommHandlerContext);

    /// Handle comm close. Default is no-op.
    fn handle_close(&mut self, _ctx: &CommHandlerContext) {}

    /// Called when the environment changes. The `event` indicates what
    /// triggered the change so handlers can decide whether to react.
    /// Default is no-op.
    fn handle_environment(&mut self, _event: &EnvironmentChanged, _ctx: &CommHandlerContext) {}
}

/// Why the environment changed.
#[derive(Debug, Clone)]
pub enum EnvironmentChanged {
    /// A top-level execution completed (user code, debug eval, etc.).
    /// Carries the current prompt state so the UI comm can forward it
    /// to the frontend.
    Execution {
        input_prompt: String,
        continuation_prompt: String,
    },
    /// The user selected a different frame in the call stack during debugging.
    FrameSelected,
}

/// A registered comm in the Console's comm table.
pub(crate) struct ConsoleComm {
    pub(crate) handler: Box<dyn CommHandler>,
    pub(crate) ctx: CommHandlerContext,
}

/// Handle an RPC request from a `CommMsg`.
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
