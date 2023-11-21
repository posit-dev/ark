//
// polled_events.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

// Polled events aren't used on Windows, so this is a no-op
pub struct RPolledEventsSuspendedScope {}

impl RPolledEventsSuspendedScope {
    pub fn new() -> Self {
        Self {}
    }
}
