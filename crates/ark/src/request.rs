//
// request.rs
//
// Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
//
//

use std::fmt::Debug;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::wire::execute_reply::ExecuteReply;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::originator::Originator;
use crossbeam::channel::Sender;

use crate::comm_handler::CommHandler;
use crate::comm_handler::CommHandlerContext;
use crate::ui::UiCommMessage;

/// Represents requests to the primary R execution thread.
#[derive(Debug, Clone)]
pub enum RRequest {
    /// Fulfill an execution request from the frontend, producing either a
    /// Reply or an Exception
    ExecuteCode(
        ExecuteRequest,
        Originator,
        Sender<amalthea::Result<ExecuteReply>>,
    ),

    /// Shut down the R execution thread
    Shutdown(bool),

    /// Commands from the debugger frontend
    DebugCommand(DebugRequest),
}

#[derive(Debug, Clone)]
pub enum DebugRequest {
    Continue,
    Next,
    StepIn,
    StepOut,
    Quit,
}

pub fn debug_request_command(req: DebugRequest) -> String {
    String::from(match req {
        DebugRequest::Continue => "c",
        DebugRequest::Next => "n",
        DebugRequest::StepIn => "s",
        DebugRequest::StepOut => "f",
        DebugRequest::Quit => "Q",
    })
}

/// Newtype mainly so `KernelRequest` can derive `Debug` despite holding a closure.
pub struct CommHandlerFactory(pub Box<dyn FnOnce() -> Box<dyn CommHandler> + Send>);

impl Debug for CommHandlerFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CommHandlerFactory(..)")
    }
}

/// Represents requests to the kernel.
#[derive(Debug)]
pub enum KernelRequest {
    /// Establish a channel to the UI comm which forwards messages to the frontend
    EstablishUiCommChannel(Sender<UiCommMessage>),

    /// Register a new comm handler on the R thread (frontend-initiated comms).
    /// Uses a factory closure so the handler (which may hold `RObject`s) is
    /// created on the R thread rather than sent across threads.
    CommOpen {
        comm_id: String,
        comm_name: String,
        factory: CommHandlerFactory,
        ctx: CommHandlerContext,
        done_tx: Sender<()>,
    },

    /// Deliver an incoming comm message to the R thread
    CommMsg {
        comm_id: String,
        msg: CommMsg,
        done_tx: Sender<()>,
    },

    /// Notify the R thread that a comm has been closed by the frontend
    CommClose {
        comm_id: String,
        done_tx: Sender<()>,
    },
}
