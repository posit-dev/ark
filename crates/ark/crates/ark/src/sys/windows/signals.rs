/*
 * signals.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use libr::Rboolean_FALSE;
use libr::Rboolean_TRUE;
use libr::UserBreak;

pub fn initialize_signal_handlers() {
    // Nothing to do on Windows. Signal blocking is POSIX only.
}

pub fn initialize_signal_block() {
    // Nothing to do on Windows. Signal blocking is POSIX only.
}

pub fn interrupts_pending() -> bool {
    unsafe { libr::get(UserBreak) == Rboolean_TRUE }
}

pub fn set_interrupts_pending(pending: bool) {
    if pending {
        unsafe { libr::set(UserBreak, Rboolean_TRUE) };
    } else {
        unsafe { libr::set(UserBreak, Rboolean_FALSE) };
    }
}
