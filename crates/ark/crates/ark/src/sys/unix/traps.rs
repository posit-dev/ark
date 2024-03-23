/*
 * traps.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use crate::traps::backtrace_handler;

pub fn register_trap_handlers() {
    unsafe {
        libc::signal(libc::SIGSEGV, backtrace_handler as libc::sighandler_t);
        libc::signal(libc::SIGILL, backtrace_handler as libc::sighandler_t);
        libc::signal(libc::SIGBUS, backtrace_handler as libc::sighandler_t);
    }
}
