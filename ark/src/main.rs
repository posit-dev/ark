/*
 * main.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use libc::{c_char, c_int, c_void};
use std::ffi::CString;

#[link(name = "R", kind = "dylib")]
extern "C" {
    // TODO: this is actually a vector of cstrings
    fn Rf_initialize_R(ac: c_int, av: *const c_char) -> i32;

    /// Global indicating whether R is running as the main program (affects
    /// R_CStackStart)
    static mut R_running_as_main_program: c_int;

    /// Flag indicating whether this is an interactive session. R typically sets
    /// this when attached to a tty.
    static mut R_Interactive: c_int;

    /// Pointer to file receiving console input
    static mut R_Consolefile: *const c_void;

    /// Pointer to file receiving output
    static mut R_Outputfile: *const c_void;

    // TODO: type of buffer isn't necessary c_char
    static mut ptr_R_ReadConsole:
        unsafe extern "C" fn(*const c_char, *const c_char, i32, i32) -> i32;
}

#[no_mangle]
pub extern "C" fn r_read_console(
    prompt: *const c_char,
    buf: *const c_char,
    buflen: i32,
    hist: i32,
) -> i32 {
    0
}

fn main() {
    let args = CString::new("").unwrap();
    // TODO: Discover R locations and populate R_HOME, a prerequisite to
    // initializing R.
    //
    // Maybe add a command line option to specify the path to R_HOME directly?
    // Or allow R_HOME to be read from the environmnet?
    unsafe {
        R_running_as_main_program = 1;
        R_Interactive = 1;
        R_Consolefile = std::ptr::null();
        R_Outputfile = std::ptr::null();
        ptr_R_ReadConsole = r_read_console;
        Rf_initialize_R(0, args.as_ptr());
    }
    println!("Hello, world.")
}
