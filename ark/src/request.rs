// 
// request.rs
// 
// Copyright (C) 2022 by RStudio, PBC
// 
// 

use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_response::ExecuteResponse;
use amalthea::wire::input_request::ShellInputRequest;
use std::sync::mpsc::{Sender, SyncSender};

/// Represents requests to the primary R execution thread.
pub enum Request {
    /// Fulfill an execution request from the front end, producing either a
    /// Reply or an Exception
    ExecuteCode(ExecuteRequest, Vec<u8>, Sender<ExecuteResponse>),

    /// Establish a channel to the front end to send input requests
    EstablishInputChannel(SyncSender<ShellInputRequest>),

    /// Shut down the R execution thread
    Shutdown(bool),
}
