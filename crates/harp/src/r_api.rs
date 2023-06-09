//
// r_api.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

#![allow(non_snake_case)]

pub use libR_sys::SEXP;

#[harp_macros::lock]
pub fn Rf_xlength(x: SEXP) -> isize {
    unsafe { libR_sys::Rf_xlength(x) }
}

#[harp_macros::lock]
pub fn TYPEOF(x: SEXP) -> std::os::raw::c_int {
    unsafe { libR_sys::TYPEOF(x) }
}

#[harp_macros::lock]
pub fn Rf_protect(x: SEXP) -> SEXP {
    unsafe { libR_sys::Rf_protect(x) }
}

#[harp_macros::lock]
pub fn Rf_unprotect(count: i32) {
    unsafe { libR_sys::Rf_unprotect(count) }
}
