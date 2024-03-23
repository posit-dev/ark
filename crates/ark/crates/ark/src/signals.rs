/*
 * signals.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

pub use crate::sys::signals::initialize_signal_block;
pub use crate::sys::signals::initialize_signal_handlers;
pub use crate::sys::signals::interrupts_pending;
pub use crate::sys::signals::set_interrupts_pending;
