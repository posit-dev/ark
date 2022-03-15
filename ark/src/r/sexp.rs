/*
 * sexp.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::r;
use libc::c_char;
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

    pub fn translate(&self, utf8: bool) -> String {
        if utf8 {
            if self.char_ce() == r::internals::CeType::CE_UTF8 {
                // TODO
                String::new()
            } else {
                let cstr = CString::from_raw(r::internals::Rf_translateCharUTF8(self.sexp));
                cstr.into_string().unwrap()
            }
        } else {
            if self.char_ce() == r::internals::CeType::CE_NATIVE {
                // TODO
                String::new()
            } else {
                let cstr = CString::from_raw(r::internals::Rf_translateChar(self.sexp));
                cstr.into_string().unwrap()
            }
        }
    }

    /// The S-expression's character encoding type
    pub fn char_ce(&self) -> r::internals::CeType {
        let kind = r::internals::Rf_getCharCE(self.sexp);
        match num::FromPrimitive::from_i32(kind) {
            Some(ce) => ce,
            None => r::internals::CeType::CE_ANY,
        }
    }

    pub fn string_elt(&self) -> Self {
        // TODO
    }

    /// Coerce the S-expression to a character type
    pub fn as_char(&self) -> Self {
        Sexp::from(r::internals::Rf_asChar(self.sexp))
    }

    /// The S-expression's primary class (from its `class` attribute)
    pub fn class(&self) -> String {
        self.attrib_string(String::from("class"))
    }

    /// Return the string value of an attribute
    pub fn attrib_string(&self, attr: String) -> String {
        // Prepare string for consumption in R
        let attr = CString::new(attr).unwrap();
        let attr_sexp = r::internals::Rf_install(attr.as_ptr());

        // Extract attribute string
        let result_sexp = r::internals::Rf_getAttrib(self.sexp, attr_sexp);
        String::from(Sexp::from(result_sexp))
    }

    // --- Private ---

    /// Returns a pointer to the raw character data in the S-expression.
    fn char(&self) -> *mut c_char {
        // TODO: should return a CHAR
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
            CHARSXP => sexp.translate(true),
            STRSXP => match sexp.length() {
                0 => String::new(),
                _ => sexp.string_elt().translate(true),
            },
            _ => {
                // translate
                String::new()
            }
        }
    }
}
