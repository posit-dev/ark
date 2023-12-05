//
// request.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_response::ExecuteResponse;
use amalthea::wire::originator::Originator;
use crossbeam::channel::Sender;

use crate::frontend::frontend::PositronFrontendMessage;

/// Represents requests to the primary R execution thread.
#[derive(Debug, Clone)]
pub enum RRequest {
    /// Fulfill an execution request from the front end, producing either a
    /// Reply or an Exception
    ExecuteCode(ExecuteRequest, Option<Originator>, Sender<ExecuteResponse>),

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
    /// Establish a channel to the front end to send events
    EstablishFrontendChannel(Sender<PositronFrontendMessage>),
}
