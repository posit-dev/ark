/*
 * console.rs
 *
 * Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
 *
 */

use std::ffi::c_char;
use std::ffi::CStr;
use std::sync::Condvar;
use std::sync::Mutex;

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

use crate::console::r_busy;
use crate::console::r_polled_events;
use crate::console::r_read_console;
use crate::console::r_show_message;
use crate::console::r_suicide;
use crate::console::r_write_console;
use crate::console::Console;
use crate::signals::initialize_signal_handlers;

// For shutdown signal in integration tests
pub static CLEANUP_SIGNAL: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

pub fn setup_r(args: &Vec<String>) {
    unsafe {
        // Before `Rf_initialize_R()`
        libr::set(R_running_as_main_program, 1);

        libr::set(R_SignalHandlers, 0);

        let mut c_args = Console::build_ark_c_args(args);
        Rf_initialize_R(c_args.len() as i32, c_args.as_mut_ptr() as *mut *mut c_char);

        // Initialize the signal blocks and handlers (like interrupts).
        // Don't do that in tests because that makes them uninterruptible.
        if !stdext::IS_TESTING {
            initialize_signal_handlers();
        }

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

        // Install a CleanUp hook for integration tests that test the shutdown process.
        // We confirm that shutdown occurs by waiting in the test until `CLEANUP_SIGNAL`'s
        // condition variable sends a notification, which occurs in this cleanup method
        // that is called during R's shutdown process.
        if stdext::IS_TESTING {
            libr::set(libr::ptr_R_CleanUp, Some(r_cleanup_for_tests));
        }

        // In tests R may be run from various threads. This confuses R's stack
        // overflow checks so we disable those. This should not make it in
        // production builds as it causes stack overflows to crash R instead of
        // throwing an R error.
        //
        // This must be called _after_ `Rf_initialize_R()`, since that's where R
        // detects the stack size and sets the default limit.
        if stdext::IS_TESTING {
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
        //
        // Note that the later package also adds an input handler to `R_InputHandlers`
        // which runs the later event loop, so it's also important that we are fairly
        // responsive for that as well (posit-dev/positron#7235).
        let mut fdset = R_checkActivity(0, 1);

        while fdset != std::ptr::null_mut() {
            R_runHandlers(libr::get(R_InputHandlers), fdset);
            fdset = R_checkActivity(0, 1);
        }
    }
}

#[cfg_attr(not(test), no_mangle)]
pub extern "C-unwind" fn r_cleanup_for_tests(_save_act: i32, _status: i32, _run_last: i32) {
    // Signal that cleanup has started
    let (lock, cvar) = &CLEANUP_SIGNAL;

    let mut started = lock.lock().unwrap();
    *started = true;

    cvar.notify_all();
    drop(started);

    // Sleep to give tests time to complete before we panic
    std::thread::sleep(std::time::Duration::from_secs(5));

    // Fallthrough to R which will call `exit()`. Note that panicking from here
    // would be UB, we can't panic over a C stack.
}
/// On Unix, we assume that the buffer to write to the console is
/// already in UTF-8
pub fn console_to_utf8(x: *const c_char) -> anyhow::Result<String> {
    let x = unsafe { CStr::from_ptr(x) };

    let x = match x.to_str() {
        Ok(content) => content,
        Err(err) => panic!("Failed to read from R buffer: {err:?}"),
    };

    Ok(x.to_string())
}
