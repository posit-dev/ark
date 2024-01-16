//
// polled_events.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use libr::R_PolledEvents_get;
use libr::R_PolledEvents_set;

static mut R_POLLED_EVENTS_OLD: Option<unsafe extern "C" fn()> = None;
static mut R_POLLED_EVENTS_SUSPENDED: i32 = 0;

pub struct RPolledEventsSuspendedScope {}

impl RPolledEventsSuspendedScope {
    pub fn new() -> Self {
        unsafe {
            if R_POLLED_EVENTS_SUSPENDED == 0 {
                R_POLLED_EVENTS_OLD = R_PolledEvents_get();
                R_PolledEvents_set(Some(r_polled_events_disabled));
            }
            R_POLLED_EVENTS_SUSPENDED += 1;
        }

        Self {}
    }
}

impl Drop for RPolledEventsSuspendedScope {
    fn drop(&mut self) {
        unsafe {
            R_POLLED_EVENTS_SUSPENDED -= 1;

            if R_POLLED_EVENTS_SUSPENDED == 0 {
                R_PolledEvents_set(R_POLLED_EVENTS_OLD);
            }
        }
    }
}

#[no_mangle]
extern "C" fn r_polled_events_disabled() {}
