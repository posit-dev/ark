//
// lib.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(improper_ctypes)]

pub use libR_sys::pDevDesc;
pub use libR_sys::pGEcontext;
pub use libR_sys::GEcurrentDevice;
pub use libR_sys::GEinitDisplayList;
pub use libR_sys::R_GE_getVersion;
pub use libR_sys::R_xlen_t;
pub use libR_sys::Rbyte;
pub use libR_sys::Rcomplex;
pub use libR_sys::Rf_error;
pub use libR_sys::Rf_errorcall;
pub use libR_sys::SEXP;
pub use libR_sys::SEXPREC;
