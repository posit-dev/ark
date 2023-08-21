//
// request.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use amalthea::events::PositronEvent;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_response::ExecuteResponse;
use amalthea::wire::originator::Originator;
use crossbeam::channel::Sender;

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
    Next,
    StepIn,
    StepOut,
    Quit,
}

/// Represents requests to the kernel.
#[derive(Debug, Clone)]
pub enum KernelRequest {
    /// Establish a channel to the front end to send events
    EstablishEventChannel(Sender<PositronEvent>),

    /// Deliver an event to the front end
    DeliverEvent(PositronEvent),
}
