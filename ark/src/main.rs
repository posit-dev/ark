/*
 * main.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use libc::{c_char, c_int};
use std::ffi::CString;

#[link(name = "libR")]
extern "C" {
    fn Rf_initialize_R(ac: c_int, av: *const c_char) -> i32;
    // static foo_global: c_int;
}

fn main() {
    let args = CString::new("").unwrap();
    unsafe {
        Rf_initialize_R(0, args.as_ptr());
    }
    println!("Hello, world.")
}
