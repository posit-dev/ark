/*
 * sexp.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::r;
use libc::{c_char, c_int};
use log::warn;
use std::convert::From;
use std::ffi::CString;

/// Thin Rust wrapper for R's S-expressions (SEXP)
struct Sexp {
    sexp: r::internals::SEXP,
}

impl Sexp {
    /// The length of the S-expression
    pub fn length(&self) -> usize {
        unsafe { r::internals::Rf_length(self.sexp) as usize }
    }

    /// The internal (R) type of the S-expression
    pub fn kind(&self) -> r::internals::SexpType {
        let sexptype = unsafe { r::internals::TYPEOF(self.sexp) };
        match num::FromPrimitive::from_i32(sexptype) {
            Some(kind) => kind,
            None => {
                warn!("Unknown SEXP type {}!", sexptype);
                r::internals::SexpType::NILSXP
            }
        }
    }

    /// Translate the S-expression to a character string. Generally avoid in
    /// favor of `String::from(sexp)`, which handles more types gracefully.
    pub fn translate(&self, utf8: bool) -> String {
        if utf8 {
            if self.char_ce() == r::internals::CeType::CE_UTF8 {
                // TODO
                String::new()
            } else {
                let cstr =
                    unsafe { CString::from_raw(r::internals::Rf_translateCharUTF8(self.sexp)) };
                cstr.into_string().unwrap()
            }
        } else {
            if self.char_ce() == r::internals::CeType::CE_NATIVE {
                // TODO
                String::new()
            } else {
                let cstr = unsafe { CString::from_raw(r::internals::Rf_translateChar(self.sexp)) };
                cstr.into_string().unwrap()
            }
        }
    }

    /// The S-expression's character encoding type
    pub fn char_ce(&self) -> r::internals::CeType {
        let kind = unsafe { r::internals::Rf_getCharCE(self.sexp) };
        match num::FromPrimitive::from_i32(kind) {
            Some(ce) => ce,
            None => r::internals::CeType::CE_ANY,
        }
    }

    /// Whether or not this S-expression is an alternative representation
    /// (ALTREP)
    pub fn altrep(&self) -> bool {
        match unsafe { self.sexp.as_ref() } {
            Some(s) => s.alt() == 1,
            None => false,
        }
    }

    /// Coerce the S-expression to a character type
    pub fn as_char(&self) -> Self {
        Sexp::from(unsafe { r::internals::Rf_asChar(self.sexp) })
    }

    /// The S-expression's primary class (from its `class` attribute)
    pub fn class(&self) -> String {
        self.attrib_string(String::from("class"))
    }

    /// Return the string value of an attribute
    pub fn attrib_string(&self, attr: String) -> String {
        // Prepare string for consumption in R
        let attr = CString::new(attr).unwrap();
        let attr_sexp = unsafe { r::internals::Rf_install(attr.as_ptr()) };

        // Extract attribute string
        let result_sexp = unsafe { r::internals::Rf_getAttrib(self.sexp, attr_sexp) };
        String::from(Sexp::from(result_sexp))
    }

    /// Extract S-expression containing string data
    pub fn string_elt(&self, offset: u32) -> Self {
        Self {
            sexp: unsafe { r::internals::STRING_ELT(self.sexp, offset as c_int) },
        }
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
            r::internals::SexpType::CHARSXP => sexp.translate(true),
            r::internals::SexpType::STRSXP => match sexp.length() {
                0 => String::new(),
                _ => sexp.string_elt(0).translate(true),
            },
            _ => {
                // translate
                String::new()
            }
        }
    }
}
