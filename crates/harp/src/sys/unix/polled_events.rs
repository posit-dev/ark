//
// polled_events.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

pub struct RLocalPolledEventsSuspended {
    _raii: crate::raii::RLocal<Option<unsafe extern "C" fn()>>,
}

#[no_mangle]
extern "C" fn r_polled_events_disabled() {}

impl RLocalPolledEventsSuspended {
    pub fn new(value: bool) -> Self {
        let new_value = if value {
            Some(r_polled_events_disabled as unsafe extern "C" fn())
        } else {
            None
        };
        Self {
            _raii: crate::raii::RLocal::new(new_value, unsafe { libr::R_PolledEvents }),
        }
    }
}
