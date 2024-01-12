//
// lib.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

mod constant_globals;
mod functions;
mod mutable_globals;

// ---------------------------------------------------------------------------------------

/// Initialization function that must be called before using any functions or globals
/// exported by the crate
pub fn initialize(library: &libloading::Library) {
    self::constant_globals_initializer::initialize(library);
    self::functions_initializer::initialize(library);
    self::mutable_globals_initializer::initialize(library);
}

pub mod has {
    pub use crate::constant_globals_has::*;
    pub use crate::functions_has::*;
    pub use crate::mutable_globals_has::*;
}

// ---------------------------------------------------------------------------------------
// Types

// Currently just using libR for the _types_, otherwise we conflict with it
pub use libR_shim::Rboolean;
pub use libR_shim::Rboolean_FALSE;
pub use libR_shim::Rboolean_TRUE;
pub use libR_shim::SEXP;

// pub type SEXPTYPE = std::ffi::c_uint;
//
// #[repr(C)]
// #[derive(Debug, Copy, Clone)]
// pub struct SEXPREC {
//     _unused: [u8; 0],
// }
// pub type SEXP = *mut SEXPREC;
//
// pub type Rboolean = u32;
// pub const Rboolean_FALSE: Rboolean = 0;
// pub const Rboolean_TRUE: Rboolean = 1;

// ---------------------------------------------------------------------------------------
// Functions and globals

functions::generate! {
    /// R >= 4.2.0
    pub fn R_existsVarInFrame(rho: SEXP, symbol: SEXP) -> Rboolean;
}

constant_globals::generate! {
    #[default = std::ptr::null_mut()]
    pub static R_NilValue: SEXP;
}

mutable_globals::generate! {
    pub static mut R_interrupts_suspended: Rboolean;
}
