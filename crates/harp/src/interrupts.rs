//
// interrupts.rs
//
// Copyright (C) 2022 by Posit Software, PBC
//
//

use libR_sys::*;

use crate::exec::r_polled_events_disabled;
use crate::exec::R_PolledEvents;

pub struct RSandboxScope {
    _interrupts_scope: RInterruptsSuspendedScope,
    _polled_events_scope: RPolledEventsSuspendedScope,
}

impl RSandboxScope {
    pub fn new() -> RSandboxScope {
        RSandboxScope {
            _interrupts_scope: RInterruptsSuspendedScope::new(),
            _polled_events_scope: RPolledEventsSuspendedScope::new(),
        }
    }
}

static mut R_INTERRUPTS_SUSPENDED: i32 = 0;

pub struct RInterruptsSuspendedScope {
    suspended: Rboolean,
}

impl RInterruptsSuspendedScope {
    pub fn new() -> RInterruptsSuspendedScope {
        unsafe {
            let suspended = R_interrupts_suspended;
            R_interrupts_suspended = 1;
            R_INTERRUPTS_SUSPENDED += 1;

            RInterruptsSuspendedScope { suspended }
        }
    }
}

impl Drop for RInterruptsSuspendedScope {
    fn drop(&mut self) {
        unsafe {
            R_INTERRUPTS_SUSPENDED -= 1;
            if R_INTERRUPTS_SUSPENDED == 0 {
                R_interrupts_suspended = self.suspended;
            }
        }
    }
}

static mut R_POLLED_EVENTS_OLD: Option<unsafe extern "C" fn()> = None;
static mut R_POLLED_EVENTS_SUSPENDED: i32 = 0;

pub struct RPolledEventsSuspendedScope {}

impl RPolledEventsSuspendedScope {
    pub fn new() -> RPolledEventsSuspendedScope {
        unsafe {
            if R_POLLED_EVENTS_SUSPENDED == 0 {
                R_POLLED_EVENTS_OLD = R_PolledEvents;
                R_PolledEvents = Some(r_polled_events_disabled);
            }
            R_POLLED_EVENTS_SUSPENDED += 1;
        }

        RPolledEventsSuspendedScope {}
    }
}

impl Drop for RPolledEventsSuspendedScope {
    fn drop(&mut self) {
        unsafe {
            R_POLLED_EVENTS_SUSPENDED -= 1;

            if R_POLLED_EVENTS_SUSPENDED == 0 {
                R_PolledEvents = R_POLLED_EVENTS_OLD;
            }
        }
    }
}
