/*
 * interface.rs
 *
 * Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *
 */

use std::ffi::CStr;
use std::mem::MaybeUninit;
use std::os::raw::c_char;

use libR_shim::run_Rmainloop;
use libR_shim::setup_Rmainloop;
use libR_shim::R_HomeDir;
use libR_shim::R_SignalHandlers;
use stdext::cargs;
use stdext::cstr;

use crate::interface::r_busy;
use crate::interface::r_read_console;
use crate::interface::r_show_message;
use crate::interface::r_write_console;
use crate::signals;
use crate::sys::windows::interface_types;

pub fn setup_r(mut _args: Vec<*mut c_char>) {
    unsafe {
        R_SignalHandlers = 0;

        // setup command line options
        // note that R does a lot of initialization here that's not accessible
        // in any other way; e.g. the default translation domain is set within
        //
        // https://github.com/rstudio/rstudio/issues/10308
        let rargc: i32 = 1;
        let mut rargv: Vec<*mut c_char> = cargs!["R.exe"];
        cmdlineoptions(rargc, rargv.as_mut_ptr() as *mut *mut c_char);

        let mut params_struct = MaybeUninit::uninit();
        let params: interface_types::Rstart = params_struct.as_mut_ptr();

        //R_DefParamsEx(params, interface_types::RSTART_VERSION as i32);
        R_DefParamsEx(params, 0);

        (*params).R_Interactive = 1;
        (*params).CharacterMode = interface_types::UImode_RGui;

        (*params).WriteConsole = None;
        (*params).WriteConsoleEx = Some(r_write_console);
        (*params).ReadConsole = Some(r_read_console);
        (*params).ShowMessage = Some(r_show_message);
        (*params).Busy = Some(r_busy);

        // This is assigned to `ptr_ProcessEvents` (which we don't set on Unix),
        // in `R_SetParams()` by `R_SetWin32()` and gets called by `R_ProcessEvents()`.
        // It gets called unconditionally, so we have to set it to something, even if a no-op.
        (*params).CallBack = Some(r_callback);

        // These need to be set before `R_SetParams()` because it accesses them, but how?
        let r_home = cstr!("C:\\Program Files\\R\\R-4.3.2");
        let user_home = cstr!("D:\\Users\\davis-vaughan\\Documents");
        (*params).rhome = r_home;
        (*params).home = user_home;

        // Sets the parameters to internal R globals, like all of the `ptr_*` function pointers
        R_SetParams(params);

        // R global ui initialization
        GA_initapp(0, std::ptr::null_mut());
        readconsolecfg();

        // Initialize the signal handlers (like interrupts)
        signals::initialize_signal_handlers();

        // Log the value of R_HOME, so we can know if something hairy is afoot
        let home = CStr::from_ptr(R_HomeDir());
        log::trace!("R_HOME: {:?}", home);

        // Set up main loop
        setup_Rmainloop();
    }
}

pub fn run_r() {
    unsafe {
        run_Rmainloop();
    }
}

pub fn run_activity_handlers() {
    // Nothing to do on Windows
}

#[no_mangle]
extern "C" fn r_callback() {
    // Do nothing!
}

extern "C" {
    fn cmdlineoptions(ac: i32, av: *mut *mut ::std::os::raw::c_char);

    fn readconsolecfg();

    fn R_DefParamsEx(Rp: interface_types::Rstart, RstartVersion: i32);

    fn R_SetParams(Rp: interface_types::Rstart);
}

// It doesn't seem like we can use the binding provided by libR-sys,
// as that doesn't link to Rgraphapp so it becomes an unresolved
// external symbol.
#[link(name = "Rgraphapp", kind = "dylib")]
extern "C" {
    pub fn GA_initapp(
        arg1: ::std::os::raw::c_int,
        arg2: *mut *mut ::std::os::raw::c_char,
    ) -> ::std::os::raw::c_int;
}
