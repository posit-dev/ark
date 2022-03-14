/*
 * sexp.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::r;
use std::convert::From;
use std::ffi::CString;

/// Thin Rust wrapper for R's S-expressions (SEXP)
struct Sexp {
    sexp: r::internals::SEXP,
}

impl Sexp {
    /// The length of the S-expression
    pub fn length(&self) -> usize {
        r::internals::Rf_length(self.sexp) as usize
    }

    /// The internal (R) type of the S-expression
    pub fn kind(&self) -> r::internals::SexpType {
        // TODO: should return a TYPEOF
    }

    /// The S-expression's primary class (from its `class` attribute)
    pub fn class(&self) -> String {
        let class = CString::new("class").unwrap();
        let class_sexp = r::internals::Rf_install(class.as_ptr());
    }
}

impl From<r::internals::SEXP> for Sexp {
    fn from(sexp: r::internals::SEXP) -> Self {
        Sexp { sexp: sexp }
    }
}

impl From<Sexp> for String {
    fn from(sexp: Sexp) -> Self {
        match sexp.kind() {
            CHARSXP => {
                // translate
                String::new()
            }
            STRSXP => {
                // translate
                String::new()
            }
            _ => {
                // translate
                String::new()
            }
        }
    }
}
