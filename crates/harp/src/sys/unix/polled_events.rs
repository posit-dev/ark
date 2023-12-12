//
// polled_events.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

static mut R_POLLED_EVENTS_OLD: Option<unsafe extern "C" fn()> = None;
static mut R_POLLED_EVENTS_SUSPENDED: i32 = 0;

pub struct RPolledEventsSuspendedScope {}

impl RPolledEventsSuspendedScope {
    pub fn new() -> Self {
        unsafe {
            if R_POLLED_EVENTS_SUSPENDED == 0 {
                R_POLLED_EVENTS_OLD = R_PolledEvents;
                R_PolledEvents = Some(r_polled_events_disabled);
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
                R_PolledEvents = R_POLLED_EVENTS_OLD;
            }
        }
    }
}

extern "C" {
    static mut R_PolledEvents: Option<unsafe extern "C" fn()>;
}

#[no_mangle]
extern "C" fn r_polled_events_disabled() {}
