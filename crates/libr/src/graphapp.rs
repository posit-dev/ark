//
// graphapp.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

// This file is Windows specific

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use crate::functions;

// ---------------------------------------------------------------------------------------
// Functions and globals

functions::generate! {
    pub fn GA_initapp(arg1: std::ffi::c_int, arg2: *mut *mut std::ffi::c_char) -> std::ffi::c_int;
}
