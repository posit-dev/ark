/*
 * signals.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use libR_shim::Rboolean_FALSE;
use libR_shim::Rboolean_TRUE;

pub fn initialize_signal_handlers() {
    // TODO: Windows
}

pub fn initialize_signal_block() {
    // TODO: Windows
}

pub fn interrupts_pending() -> bool {
    unsafe { UserBreak == Rboolean_TRUE }
}

pub fn set_interrupts_pending(pending: bool) {
    if pending {
        unsafe { UserBreak = Rboolean_TRUE };
    } else {
        unsafe { UserBreak = Rboolean_FALSE };
    }
}

#[link(name = "R", kind = "dylib")]
extern "C" {
    static mut UserBreak: libR_shim::Rboolean;
}
