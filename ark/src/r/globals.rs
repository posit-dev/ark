/*
 * globals.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use libc::{c_char, c_int, c_void};

#[link(name = "R", kind = "dylib")]
extern "C" {
    /// Global indicating whether R is running as the main program (affects
    /// R_CStackStart)
    pub static mut R_running_as_main_program: c_int;

    /// Flag indicating whether this is an interactive session. R typically sets
    /// this when attached to a tty.
    pub static mut R_Interactive: c_int;

    /// Pointer to file receiving console input
    pub static mut R_Consolefile: *const c_void;

    /// Pointer to file receiving output
    pub static mut R_Outputfile: *const c_void;

    /// Signal handlers for R
    pub static mut R_SignalHandlers: c_int;

    // TODO: type of buffer isn't necessary c_char
    pub static mut ptr_R_ReadConsole:
        unsafe extern "C" fn(*mut c_char, *mut c_char, c_int, c_int) -> c_int;

    /// Pointer to console write function
    pub static mut ptr_R_WriteConsole: *const c_void;

    /// Pointer to extended console write function
    pub static mut ptr_R_WriteConsoleEx: unsafe extern "C" fn(*mut c_char, c_int, c_int);
}
