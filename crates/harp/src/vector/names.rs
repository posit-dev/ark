//
// names.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//
use libr::SEXP;

use crate::object::RObject;
use crate::vector::CharacterVector;
use crate::vector::Vector;

pub struct Names {
    data: Option<CharacterVector>,
    default: Box<dyn Fn(isize) -> String>,
}

impl Names {
    pub fn new(x: SEXP, default: impl Fn(isize) -> String + 'static) -> Self {
        unsafe {
            let names = RObject::view(x).get_attribute_names();
            let default = Box::new(default);
            match names {
                Some(names) => Self {
                    data: Some(CharacterVector::new_unchecked(names.sexp)),
                    default,
                },
                None => Self {
                    data: None,
                    default,
                },
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
