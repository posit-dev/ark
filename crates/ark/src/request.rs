//
// request.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use amalthea::events::PositronEvent;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_response::ExecuteResponse;
use amalthea::wire::input_request::ShellInputRequest;
use amalthea::wire::originator::Originator;
use crossbeam::channel::Sender;

/// Represents requests to the primary R execution thread.
#[derive(Debug, Clone)]
pub enum Request {
    /// Fulfill an execution request from the front end, producing either a
    /// Reply or an Exception
    ExecuteCode(ExecuteRequest, Option<Originator>, Sender<ExecuteResponse>),

    /// Establish a channel to the front end to send input requests
    EstablishInputChannel(Sender<ShellInputRequest>),

    /// Establish a channel to the front end to send events
    EstablishEventChannel(Sender<PositronEvent>),

    /// Deliver an event to the front end
    DeliverEvent(PositronEvent),

    /// Shut down the R execution thread
    Shutdown(bool),
}
