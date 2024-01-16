/*
 * interface.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use std::ffi::CStr;
use std::os::raw::c_char;

use libr::ptr_R_Busy_set;
use libr::ptr_R_ReadConsole_set;
use libr::ptr_R_ShowMessage_set;
use libr::ptr_R_WriteConsoleEx_set;
use libr::ptr_R_WriteConsole_set;
use libr::run_Rmainloop;
use libr::setup_Rmainloop;
use libr::R_Consolefile_set;
use libr::R_HomeDir;
use libr::R_InputHandlers_get;
use libr::R_Interactive_set;
use libr::R_Outputfile_set;
use libr::R_PolledEvents_set;
use libr::R_SignalHandlers_set;
use libr::R_checkActivity;
use libr::R_runHandlers;
use libr::R_running_as_main_program_set;
use libr::R_wait_usec_set;
use libr::Rf_initialize_R;

use crate::interface::r_busy;
use crate::interface::r_polled_events;
use crate::interface::r_read_console;
use crate::interface::r_show_message;
use crate::interface::r_write_console;
use crate::signals::initialize_signal_handlers;

pub fn setup_r(mut args: Vec<*mut c_char>) {
    unsafe {
        // Before `Rf_initialize_R()`
        R_running_as_main_program_set(1);

        R_SignalHandlers_set(0);

        Rf_initialize_R(args.len() as i32, args.as_mut_ptr() as *mut *mut c_char);

        // Initialize the signal blocks and handlers (like interrupts)
        initialize_signal_handlers();

        // Mark R session as interactive
        // (Should have also been set by call to `Rf_initialize_R()`)
        R_Interactive_set(1);

        // Log the value of R_HOME, so we can know if something hairy is afoot
        let home = CStr::from_ptr(R_HomeDir());
        log::trace!("R_HOME: {:?}", home);

        // Redirect console
        R_Consolefile_set(std::ptr::null_mut());
        R_Outputfile_set(std::ptr::null_mut());

        ptr_R_WriteConsole_set(None);
        ptr_R_WriteConsoleEx_set(Some(r_write_console));
        ptr_R_ReadConsole_set(Some(r_read_console));
        ptr_R_ShowMessage_set(Some(r_show_message));
        ptr_R_Busy_set(Some(r_busy));

        // Set up main loop
        setup_Rmainloop();
    }
}

pub fn run_r() {
    unsafe {
        // Listen for polled events
        R_wait_usec_set(10000);
        R_PolledEvents_set(Some(r_polled_events));

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
            R_runHandlers(R_InputHandlers_get(), fdset);
            fdset = R_checkActivity(0, 1);
        }
    }
}
