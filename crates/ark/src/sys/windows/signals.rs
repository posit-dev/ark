/*
 * signals.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use libr::Rboolean_FALSE;
use libr::Rboolean_TRUE;
use libr::UserBreak_get;
use libr::UserBreak_set;

pub fn initialize_signal_handlers() {
    // Nothing to do on Windows. Signal blocking is POSIX only.
}

pub fn initialize_signal_block() {
    // Nothing to do on Windows. Signal blocking is POSIX only.
}

pub fn interrupts_pending() -> bool {
    unsafe { UserBreak_get() == Rboolean_TRUE }
}

pub fn set_interrupts_pending(pending: bool) {
    if pending {
        unsafe { UserBreak_set(Rboolean_TRUE) };
    } else {
        unsafe { UserBreak_set(Rboolean_FALSE) };
    }
}
