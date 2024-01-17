//
// types.rs
//
// Copyright (C) 2024 Posit Software, PBC. All rights reserved.
//
//

#![allow(non_snake_case)]
#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

// Reexport all system specific R types
pub use crate::sys::types::*;

#[doc = "R_xlen_t is defined as int on 32-bit platforms, and that confuses Rust. Keeping it always as ptrdiff_t works fine even on 32-bit."]
pub type R_xlen_t = isize;

pub type SEXPTYPE = std::ffi::c_uint;
pub const NILSXP: u32 = 0;
pub const SYMSXP: u32 = 1;
pub const LISTSXP: u32 = 2;
pub const CLOSXP: u32 = 3;
pub const ENVSXP: u32 = 4;
pub const PROMSXP: u32 = 5;
pub const LANGSXP: u32 = 6;
pub const SPECIALSXP: u32 = 7;
pub const BUILTINSXP: u32 = 8;
pub const CHARSXP: u32 = 9;
pub const LGLSXP: u32 = 10;
pub const INTSXP: u32 = 13;
pub const REALSXP: u32 = 14;
pub const CPLXSXP: u32 = 15;
pub const STRSXP: u32 = 16;
pub const DOTSXP: u32 = 17;
pub const ANYSXP: u32 = 18;
pub const VECSXP: u32 = 19;
pub const EXPRSXP: u32 = 20;
pub const BCODESXP: u32 = 21;
pub const EXTPTRSXP: u32 = 22;
pub const WEAKREFSXP: u32 = 23;
pub const RAWSXP: u32 = 24;
pub const S4SXP: u32 = 25;
pub const NEWSXP: u32 = 30;
pub const FREESXP: u32 = 31;
pub const FUNSXP: u32 = 99;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct SEXPREC {
    _unused: [u8; 0],
}
pub type SEXP = *mut SEXPREC;

#[doc = "R 4.3 redefined `Rcomplex` to a union for compatibility with Fortran.\n But the old definition is compatible both the union version\n and the struct version.\n See: https://github.com/extendr/extendr/issues/524"]
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Rcomplex {
    pub r: f64,
    pub i: f64,
}

pub type Rboolean = u32;
pub const Rboolean_FALSE: Rboolean = 0;
pub const Rboolean_TRUE: Rboolean = 1;

pub type Rbyte = std::ffi::c_uchar;

pub type cetype_t = u32;
pub const cetype_t_CE_NATIVE: cetype_t = 0;
pub const cetype_t_CE_UTF8: cetype_t = 1;
pub const cetype_t_CE_LATIN1: cetype_t = 2;
pub const cetype_t_CE_BYTES: cetype_t = 3;
pub const cetype_t_CE_SYMBOL: cetype_t = 5;
pub const cetype_t_CE_ANY: cetype_t = 99;

pub type ParseStatus = u32;
pub const ParseStatus_PARSE_NULL: ParseStatus = 0;
pub const ParseStatus_PARSE_OK: ParseStatus = 1;
pub const ParseStatus_PARSE_INCOMPLETE: ParseStatus = 2;
pub const ParseStatus_PARSE_ERROR: ParseStatus = 3;
pub const ParseStatus_PARSE_EOF: ParseStatus = 4;

pub type DL_FUNC = Option<unsafe extern "C" fn() -> *mut std::ffi::c_void>;
pub type R_NativePrimitiveArgType = std::ffi::c_uint;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct _DllInfo {
    _unused: [u8; 0],
}
pub type DllInfo = _DllInfo;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct R_CMethodDef {
    pub name: *const std::ffi::c_char,
    pub fun: DL_FUNC,
    pub numArgs: std::ffi::c_int,
    pub types: *mut R_NativePrimitiveArgType,
}

pub type R_FortranMethodDef = R_CMethodDef;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct R_CallMethodDef {
    pub name: *const std::ffi::c_char,
    pub fun: DL_FUNC,
    pub numArgs: std::ffi::c_int,
}

pub type R_ExternalMethodDef = R_CallMethodDef;
