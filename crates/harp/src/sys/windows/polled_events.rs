//
// polled_events.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

// Polled events aren't used on Windows, so this is a no-op
pub struct RLocalPolledEventsSuspended {}

impl RLocalPolledEventsSuspended {
    pub fn new(_value: bool) -> Self {
        Self {}
    }
}
