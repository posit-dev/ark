/*
 * control.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use crate::signals::set_interrupts_pending;

pub fn handle_interrupt_request() {
    set_interrupts_pending(true);
}
