//
// request.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use amalthea::wire::execute_reply::ExecuteReply;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::originator::Originator;
use crossbeam::channel::Sender;

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

/// Represents requests to the kernel.
#[derive(Debug, Clone)]
pub enum KernelRequest {
    /// Establish a channel to the UI comm which forwards messages to the frontend
    EstablishUiCommChannel(Sender<UiCommMessage>),
}
