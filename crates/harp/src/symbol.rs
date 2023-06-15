//
// symbol.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::ffi::CStr;
use std::ops::Deref;

use libR_sys::*;

use crate::error::Result;
use crate::r_symbol;
use crate::utils::r_assert_type;
use crate::utils::Sxpinfo;
use crate::utils::HASHASH_MASK;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct RSymbol {
    pub sexp: SEXP,
}

impl RSymbol {
    pub fn new_unchecked(sexp: SEXP) -> Self {
        Self { sexp }
    }

    pub fn new(sexp: SEXP) -> Result<Self> {
        r_assert_type(sexp, &[SYMSXP])?;
        Ok(Self::new_unchecked(sexp))
    }

    pub fn has_hash(&self) -> bool {
        unsafe { (Sxpinfo::interpret(&PRINTNAME(self.sexp)).gp() & HASHASH_MASK) == 1 }
    }
}

impl Deref for RSymbol {
    type Target = SEXP;
    fn deref(&self) -> &Self::Target {
        &self.sexp
    }
}

impl From<RSymbol> for String {
    fn from(symbol: RSymbol) -> Self {
        unsafe {
            let utf8text = Rf_translateCharUTF8(PRINTNAME(*symbol));
            CStr::from_ptr(utf8text).to_str().unwrap().to_string()
        }
    }
}

impl From<&str> for RSymbol {
    fn from(value: &str) -> Self {
        RSymbol {
            sexp: unsafe { r_symbol!(value) },
        }
    }
}

impl From<&String> for RSymbol {
    fn from(value: &String) -> Self {
        RSymbol::from(value.as_str())
    }
}

impl std::fmt::Display for RSymbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", String::from(*self))
    }
}

impl PartialEq<&str> for RSymbol {
    fn eq(&self, other: &&str) -> bool {
        unsafe {
            let utf8text = Rf_translateCharUTF8(PRINTNAME(self.sexp));
            CStr::from_ptr(utf8text).to_str().unwrap() == *other
        }
    }
}
