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

/// Represents requests to the kernel.
pub enum KernelRequest {
    /// Establish a channel to the UI comm which forwards messages to the frontend
    EstablishUiCommChannel(Sender<UiCommMessage>),

    /// Register a new comm handler on the R thread (frontend-initiated comms)
    CommOpen {
        comm_id: String,
        comm_name: String,
        handler: Box<dyn CommHandler>,
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

// Manual impl to avoid requiring `Debug` as a supertrait of `CommHandler`.
impl std::fmt::Debug for KernelRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EstablishUiCommChannel(_) => f.debug_tuple("EstablishUiCommChannel").finish(),
            Self::CommOpen {
                comm_id, comm_name, ..
            } => f
                .debug_struct("CommOpen")
                .field("comm_id", comm_id)
                .field("comm_name", comm_name)
                .finish(),
            Self::CommMsg { comm_id, .. } => {
                f.debug_struct("CommMsg").field("comm_id", comm_id).finish()
            },
            Self::CommClose { comm_id, .. } => f
                .debug_struct("CommClose")
                .field("comm_id", comm_id)
                .finish(),
        }
    }
}
