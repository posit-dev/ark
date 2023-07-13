//
// globals.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use amalthea::comm::event::CommEvent;
use crossbeam::channel::Sender;
use parking_lot::Mutex;
use parking_lot::MutexGuard;

// The communication channel manager's request channel.
// For use within R callback functions.
static mut COMM_MANAGER_TX: Option<Mutex<Sender<CommEvent>>> = None;

pub(super) fn comm_manager_tx<'a>() -> MutexGuard<'a, Sender<CommEvent>> {
    unsafe { COMM_MANAGER_TX.as_ref().unwrap_unchecked().lock() }
}

pub fn initialize(comm_manager_tx: Sender<CommEvent>) {
    unsafe {
        COMM_MANAGER_TX = Some(Mutex::new(comm_manager_tx));
    }
}
