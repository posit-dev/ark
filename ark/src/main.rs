/*
 * main.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use libc::{c_char, c_int};
use std::ffi::CString;

#[link(name = "R", kind = "dylib")]
extern "C" {
    // TODO: this is actually a vector of cstrings
    fn Rf_initialize_R(ac: c_int, av: *const c_char) -> i32;

    /// Global indicating whether R is running as the main program (affects
    /// R_CStackStart)
    static mut R_running_as_main_program: c_int;
}

fn main() {
    let args = CString::new("").unwrap();
    unsafe {
        R_running_as_main_program = 1;
        Rf_initialize_R(0, args.as_ptr());
    }
    println!("Hello, world.")
}
