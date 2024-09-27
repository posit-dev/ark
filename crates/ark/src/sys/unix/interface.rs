/*
 * interface.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use std::ffi::CStr;
use std::os::raw::c_char;

use libr::ptr_R_Busy;
use libr::ptr_R_ReadConsole;
use libr::ptr_R_ShowMessage;
use libr::ptr_R_Suicide;
use libr::ptr_R_WriteConsole;
use libr::ptr_R_WriteConsoleEx;
use libr::run_Rmainloop;
use libr::setup_Rmainloop;
use libr::R_Consolefile;
use libr::R_HomeDir;
use libr::R_InputHandlers;
use libr::R_Interactive;
use libr::R_Outputfile;
use libr::R_PolledEvents;
use libr::R_SignalHandlers;
use libr::R_checkActivity;
use libr::R_runHandlers;
use libr::R_running_as_main_program;
use libr::R_wait_usec;
use libr::Rf_initialize_R;

use crate::interface::r_busy;
use crate::interface::r_polled_events;
use crate::interface::r_read_console;
use crate::interface::r_show_message;
use crate::interface::r_suicide;
use crate::interface::r_write_console;
use crate::signals::initialize_signal_handlers;

pub fn setup_r(mut args: Vec<*mut c_char>) {
    unsafe {
        // Before `Rf_initialize_R()`
        libr::set(R_running_as_main_program, 1);

        libr::set(R_SignalHandlers, 0);

        Rf_initialize_R(args.len() as i32, args.as_mut_ptr() as *mut *mut c_char);

        // Initialize the signal blocks and handlers (like interrupts)
        initialize_signal_handlers();

        // Mark R session as interactive
        // (Should have also been set by call to `Rf_initialize_R()`)
        libr::set(R_Interactive, 1);

        // Log the value of R_HOME, so we can know if something hairy is afoot
        let home = CStr::from_ptr(R_HomeDir());
        log::trace!("R_HOME: {:?}", home);

        // Redirect console
        libr::set(R_Consolefile, std::ptr::null_mut());
        libr::set(R_Outputfile, std::ptr::null_mut());

        libr::set(ptr_R_WriteConsole, None);
        libr::set(ptr_R_WriteConsoleEx, Some(r_write_console));
        libr::set(ptr_R_ReadConsole, Some(r_read_console));
        libr::set(ptr_R_ShowMessage, Some(r_show_message));
        libr::set(ptr_R_Busy, Some(r_busy));
        libr::set(ptr_R_Suicide, Some(r_suicide));

        // In tests R may be run from various threads. This confuses R's stack
        // overflow checks so we disable those. This should not make it in
        // production builds as it causes stack overflows to crash R instead of
        // throwing an R error.
        //
        // This must be called _after_ `Rf_initialize_R()`, since that's where R
        // detects the stack size and sets the default limit.
        if harp::test::IS_TESTING {
            libr::set(libr::R_CStackLimit, usize::MAX);
        }

        // Set up main loop
        setup_Rmainloop();
    }
}

pub fn run_r() {
    unsafe {
        // Listen for polled events
        libr::set(R_wait_usec, 10000);
        libr::set(R_PolledEvents, Some(r_polled_events));

        run_Rmainloop();
    }
}

pub fn run_activity_handlers() {
    unsafe {
        // Run handlers if we have data available. This is necessary
        // for things like the HTML help server, which will listen
        // for requests on an open socket() which would then normally
        // be handled in a select() call when reading input from stdin.
        //
        // https://github.com/wch/r-source/blob/4ca6439c1ffc76958592455c44d83f95d5854b2a/src/unix/sys-std.c#L1084-L1086
        //
        // We run this in a loop just to make sure the R help server can
        // be as responsive as possible when rendering help pages.
        let mut fdset = R_checkActivity(0, 1);

        while fdset != std::ptr::null_mut() {
            R_runHandlers(libr::get(R_InputHandlers), fdset);
            fdset = R_checkActivity(0, 1);
        }
    }
}
