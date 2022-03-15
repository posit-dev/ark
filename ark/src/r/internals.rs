/*
 * internals.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use libc::{c_char, c_int, c_void};

/// Types of S-expressions
#[derive(FromPrimitive, PartialEq)]
pub enum SexpType {
    /// nil = NULL
    NILSXP = 0,
    /// symbols
    SYMSXP = 1,
    /// lists of dotted pairs
    LISTSXP = 2,
    /// closures
    CLOSXP = 3,
    /// environments
    ENVSXP = 4,
    /// promises: [un]evaluated closure arguments
    PROMSXP = 5,
    /// language constructs (special lists)
    LANGSXP = 6,
    /// special forms
    SPECIALSXP = 7,
    /// builtin non-special forms
    BUILTINSXP = 8,
    /// "scalar" string type (internal only)
    CHARSXP = 9,
    /// logical vectors
    LGLSXP = 10,
    /// integer vectors
    INTSXP = 13,
    /// real variables
    REALSXP = 14,
    /// complex variables
    CPLXSXP = 15,
    /// string vectors
    STRSXP = 16,
    /// dot-dot-dot object
    DOTSXP = 17,
    /// make "any" args work.
    ANYSXP = 18,
    /// generic vectors
    VECSXP = 19,
    /// expressions vectors
    EXPRSXP = 20,
    /// byte code
    BCODESXP = 21,
    /// external pointer
    EXTPTRSXP = 22,
    /// weak reference
    WEAKREFSXP = 23,
    /// raw bytes
    RAWSXP = 24,
    /// S4, non-vector
    S4SXP = 25,
    /// fresh node created in new page
    NEWSXP = 30,
    /// node released by GC
    FREESXP = 31,
    /// Closure or Builtin or Special
    FUNSXP = 99,
}

/// Character encoding types
#[derive(FromPrimitive, PartialEq)]
pub enum CeType {
    /// Native (system) encoding
    CE_NATIVE = 0,
    /// UTF-8 encoding
    CE_UTF8 = 1,
    /// Latin1 encoding
    CE_LATIN1 = 2,
    /// Raw (bytes) encoding
    CE_BYTES = 3,
    /// Symbol encoding
    CE_SYMBOL = 5,
    /// Other
    CE_ANY = 99,
}

pub type SEXP = *const c_void;

#[link(name = "R", kind = "dylib")]
extern "C" {
    /// Install a string as an S-expression
    pub fn Rf_install(str: *const c_char) -> SEXP;

    /// Get an attribute of an S-expression
    pub fn Rf_getAttrib(obj: SEXP, attrib: SEXP) -> SEXP;

    /// Get the length of an S-expression
    pub fn Rf_length(obj: SEXP) -> c_int;

    /// Translate an S-expression to a null-terminated C string
    pub fn Rf_translateChar(obj: SEXP) -> *mut c_char;

    /// Translate an S-expression to a null-terminated C string (UTF-8)
    pub fn Rf_translateCharUTF8(obj: SEXP) -> *mut c_char;

    /// Get the type of an S-expression holding character data
    pub fn Rf_getCharCE(obj: SEXP) -> c_int;
}
