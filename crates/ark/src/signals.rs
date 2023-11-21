/*
 * signals.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use libR_sys::R_interrupts_pending;

pub use crate::sys::signals::initialize_signal_block;
pub use crate::sys::signals::initialize_signal_handlers;

pub extern "C" fn handle_interrupt(_signal: libc::c_int) {
    unsafe {
        R_interrupts_pending = 1;
    }
}
