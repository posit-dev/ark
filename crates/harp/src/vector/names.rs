//
// names.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//
use libR_shim::*;
use libr::R_NamesSymbol;

use crate::object::RObject;
use crate::utils::r_is_null;
use crate::vector::CharacterVector;
use crate::vector::Vector;

pub struct Names {
    data: Option<CharacterVector>,
    default: Box<dyn Fn(isize) -> String>,
}

impl Names {
    pub fn new(x: SEXP, default: impl Fn(isize) -> String + 'static) -> Self {
        unsafe {
            let names = RObject::new(Rf_getAttrib(x, R_NamesSymbol));
            let default = Box::new(default);
            if r_is_null(*names) {
                Self {
                    data: None,
                    default,
                }
            } else {
                Self {
                    data: Some(CharacterVector::new_unchecked(names)),
                    default,
                }
            }
        }
    }

    pub fn get_unchecked(&self, index: isize) -> String {
        match &self.data {
            // when there are no names
            None => (self.default)(index),
            Some(names) => match names.get_unchecked(index) {
                // where the name is NA
                None => (self.default)(index),

                Some(name) => {
                    if name.len() == 0 {
                        // empty name
                        (self.default)(index)
                    } else {
                        // real name
                        name
                    }
                },
            },
        }
    }
}
