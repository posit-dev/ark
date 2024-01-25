//
// polled_events.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use libr::R_PolledEvents;

static mut R_POLLED_EVENTS_OLD: Option<unsafe extern "C" fn()> = None;
static mut R_POLLED_EVENTS_SUSPENDED: i32 = 0;

pub struct RPolledEventsSuspendedScope {}

impl RPolledEventsSuspendedScope {
    pub fn new() -> Self {
        unsafe {
            if R_POLLED_EVENTS_SUSPENDED == 0 {
                R_POLLED_EVENTS_OLD = libr::get(R_PolledEvents);
                libr::set(R_PolledEvents, Some(r_polled_events_disabled));
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
                libr::set(R_PolledEvents, R_POLLED_EVENTS_OLD);
            }
        }
    }
}

#[no_mangle]
extern "C" fn r_polled_events_disabled() {}
