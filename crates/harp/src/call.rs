//
// call.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::ops::Deref;

use libR_sys::*;

use crate::error::Result;
use crate::object::RObject;
use crate::utils::r_assert_type;

pub struct RCall {
    pub object: RObject,
}

impl RCall {
    pub fn new_unchecked(object: impl Into<RObject>) -> Self {
        Self {
            object: object.into(),
        }
    }

    pub fn new(object: impl Into<RObject>) -> Result<Self> {
        let object = object.into();
        r_assert_type(*object, &[LANGSXP])?;
        Ok(Self::new_unchecked(object))
    }
}

impl Deref for RCall {
    type Target = SEXP;
    fn deref(&self) -> &Self::Target {
        &self.object
    }
}

impl From<RCall> for RObject {
    fn from(value: RCall) -> Self {
        value.object
    }
}
