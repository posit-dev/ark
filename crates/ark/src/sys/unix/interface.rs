/*
 * interface.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use std::ffi::CStr;
use std::os::raw::c_char;
use std::os::raw::c_void;

use libR_shim::setup_Rmainloop;
use libR_shim::R_SignalHandlers;
use libR_shim::Rboolean;
use libR_shim::Rf_initialize_R;
use libR_shim::FILE;

use crate::interface::r_busy;
use crate::interface::r_polled_events;
use crate::interface::r_read_console;
use crate::interface::r_show_message;
use crate::interface::r_write_console;
use crate::interface::run_Rmainloop;
use crate::interface::R_HomeDir;
use crate::signals;

pub fn setup_r(mut args: Vec<*mut c_char>) {
    unsafe {
        // Before `Rf_initialize_R()`
        R_running_as_main_program = 1;

        R_SignalHandlers = 0;

        Rf_initialize_R(args.len() as i32, args.as_mut_ptr() as *mut *mut c_char);

        // Initialize the signal handlers (like interrupts)
        signals::initialize_signal_handlers();

        // Mark R session as interactive
        // (Should have also been set by call to `Rf_initialize_R()`)
        R_Interactive = 1;

        // Log the value of R_HOME, so we can know if something hairy is afoot
        let home = CStr::from_ptr(R_HomeDir());
        log::trace!("R_HOME: {:?}", home);

        // Redirect console
        R_Consolefile = std::ptr::null_mut();
        R_Outputfile = std::ptr::null_mut();

        ptr_R_WriteConsole = None;
        ptr_R_WriteConsoleEx = Some(r_write_console);
        ptr_R_ReadConsole = Some(r_read_console);
        ptr_R_ShowMessage = Some(r_show_message);
        ptr_R_Busy = Some(r_busy);

        // Set up main loop
        setup_Rmainloop();
    }
}

pub fn run_r() {
    unsafe {
        // Listen for polled events
        R_wait_usec = 10000;
        R_PolledEvents = Some(r_polled_events);

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
            R_runHandlers(R_InputHandlers, fdset);
            fdset = R_checkActivity(0, 1);
        }
    }
}

extern "C" {
    static mut R_running_as_main_program: ::std::os::raw::c_int;
    static mut R_Interactive: Rboolean;
    static mut R_InputHandlers: *const c_void;
    static mut R_Consolefile: *mut FILE;
    static mut R_Outputfile: *mut FILE;

    static mut R_wait_usec: i32;
    static mut R_PolledEvents: Option<unsafe extern "C" fn()>;

    // NOTE: Some of these routines don't really return (or use) void pointers,
    // but because we never introspect these values directly and they're always
    // passed around in R as pointers, it suffices to just use void pointers.
    fn R_checkActivity(usec: i32, ignore_stdin: i32) -> *const c_void;
    fn R_runHandlers(handlers: *const c_void, fdset: *const c_void);

    static mut ptr_R_WriteConsole: ::std::option::Option<
        unsafe extern "C" fn(arg1: *const ::std::os::raw::c_char, arg2: ::std::os::raw::c_int),
    >;

    static mut ptr_R_WriteConsoleEx: ::std::option::Option<
        unsafe extern "C" fn(
            arg1: *const ::std::os::raw::c_char,
            arg2: ::std::os::raw::c_int,
            arg3: ::std::os::raw::c_int,
        ),
    >;

    static mut ptr_R_ReadConsole: ::std::option::Option<
        unsafe extern "C" fn(
            arg1: *const ::std::os::raw::c_char,
            arg2: *mut ::std::os::raw::c_uchar,
            arg3: ::std::os::raw::c_int,
            arg4: ::std::os::raw::c_int,
        ) -> ::std::os::raw::c_int,
    >;

    static mut ptr_R_ShowMessage:
        ::std::option::Option<unsafe extern "C" fn(arg1: *const ::std::os::raw::c_char)>;

    static mut ptr_R_Busy: ::std::option::Option<unsafe extern "C" fn(arg1: ::std::os::raw::c_int)>;
}
