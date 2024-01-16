//
// symbol.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::cmp::Ordering;
use std::ffi::CStr;
use std::ops::Deref;

use libR_shim::*;
use libr::vmaxget;
use libr::vmaxset;

use crate::error::Result;
use crate::object::r_length;
use crate::r_symbol;
use crate::utils::r_assert_type;
use crate::utils::r_str_to_owned_utf8_unchecked;
use crate::utils::Sxpinfo;
use crate::utils::HASHASH_MASK;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

impl Ord for RSymbol {
    fn cmp(&self, other: &Self) -> Ordering {
        unsafe {
            let self_char = PRINTNAME(self.sexp);
            let other_char = PRINTNAME(other.sexp);

            let self_nchar = r_length(self_char) as usize;
            let other_nchar = r_length(other_char) as usize;

            let self_slice = std::slice::from_raw_parts(R_CHAR(self_char), self_nchar);
            let other_slice = std::slice::from_raw_parts(R_CHAR(other_char), other_nchar);

            Ord::cmp(self_slice, other_slice)
        }
    }
}

impl PartialOrd for RSymbol {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
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
        unsafe { r_str_to_owned_utf8_unchecked(PRINTNAME(*symbol)) }
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
            let vmax = vmaxget();
            let utf8text = Rf_translateCharUTF8(PRINTNAME(self.sexp));
            vmaxset(vmax);
            CStr::from_ptr(utf8text).to_str().unwrap() == *other
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::r_test;
    use crate::symbol::RSymbol;

    #[test]
    fn test_rsymbol_ord() {
        r_test! {
            let mut x = vec![RSymbol::from("z"), RSymbol::from("m"), RSymbol::from("a")];
            x.sort();
            assert_eq!(x, vec![RSymbol::from("a"), RSymbol::from("m"), RSymbol::from("z")]);
        }
    }
}
