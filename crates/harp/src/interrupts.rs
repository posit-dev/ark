//
// interrupts.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use libr::R_interrupts_suspended_get;
use libr::R_interrupts_suspended_set;
use libr::Rboolean;
use libr::Rboolean_TRUE;

static mut R_INTERRUPTS_SUSPENDED: i32 = 0;

pub struct RInterruptsSuspendedScope {
    suspended: Rboolean,
}

impl RInterruptsSuspendedScope {
    pub fn new() -> RInterruptsSuspendedScope {
        unsafe {
            let suspended = R_interrupts_suspended_get();
            R_interrupts_suspended_set(Rboolean_TRUE);
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
                R_interrupts_suspended_set(self.suspended);
            }
        }
    }
}
