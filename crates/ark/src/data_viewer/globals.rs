//
// globals.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::event::CommEvent;
use crossbeam::channel::Sender;

// The global state used by R callbacks.
//
// Doesn't need a mutex because it's only accessed by the R thread. Should
// not be used elsewhere than from an R frontend callback or an R function
// invoked by the REPL.
pub(super) static mut R_CALLBACK_GLOBALS: Option<RCallbackGlobals> = None;

pub(super) struct RCallbackGlobals {
    pub(super) comm_manager_tx: Sender<CommEvent>,
}

impl RCallbackGlobals {
    fn new(comm_manager_tx: Sender<CommEvent>) -> Self {
        Self { comm_manager_tx }
    }
}

pub fn initialize(comm_manager_tx: Sender<CommEvent>) {
    unsafe {
        R_CALLBACK_GLOBALS = Some(RCallbackGlobals::new(comm_manager_tx));
    }
}
