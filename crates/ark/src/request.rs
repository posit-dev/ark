//
// request.rs
//
// Copyright (C) 2022-2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::wire::execute_reply::ExecuteReply;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::originator::Originator;
use crossbeam::channel::Sender;

use crate::ui::UiCommMessage;

/// Represents requests to the primary R execution thread.
#[derive(Debug, Clone)]
#[expect(clippy::large_enum_variant)]
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

/// Represents requests to the kernel.
#[derive(Debug)]
#[expect(clippy::large_enum_variant)]
pub enum KernelRequest {
    /// Establish a channel to the UI comm which forwards messages to the frontend
    EstablishUiCommChannel(Sender<UiCommMessage>),

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
