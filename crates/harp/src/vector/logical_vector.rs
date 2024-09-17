//
// logical_vector.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use libr::R_NaInt;
use libr::R_xlen_t;
use libr::Rf_allocVector;
use libr::DATAPTR;
use libr::LGLSXP;
use libr::SEXP;

use crate::object::RObject;
use crate::r_lgl_na;
use crate::try_lgl_get;
use crate::vector::FormatOptions;
use crate::vector::Vector;

#[harp_macros::vector]
pub struct LogicalVector {
    object: RObject,
}

impl Vector for LogicalVector {
    type Item = bool;
    type Type = bool;
    const SEXPTYPE: u32 = LGLSXP;
    type UnderlyingType = i32;
    type CompareType = bool;

    unsafe fn new_unchecked(object: impl Into<SEXP>) -> Self {
        Self {
            object: RObject::new(object.into()),
        }
    }

    unsafe fn create<T>(data: T) -> Self
    where
        T: IntoIterator,
        <T as IntoIterator>::IntoIter: ExactSizeIterator,
        <T as IntoIterator>::Item: AsRef<Self::Item>,
    {
        let it = data.into_iter();
        let count = it.len();

        let vector = Rf_allocVector(Self::SEXPTYPE, count as R_xlen_t);
        let dataptr = DATAPTR(vector) as *mut Self::Type;
        it.enumerate().for_each(|(index, value)| {
            *(dataptr.offset(index as isize)) = *value.as_ref();
        });

        Self::new_unchecked(vector)
    }

    fn data(&self) -> SEXP {
        self.object.sexp
    }

    fn is_na(x: &Self::UnderlyingType) -> bool {
        unsafe { *x == R_NaInt }
    }

    fn get_unchecked_elt(&self, index: isize) -> harp::Result<Self::UnderlyingType> {
        try_lgl_get(self.data(), R_xlen_t::from(index))
    }

    fn error_elt() -> Self::UnderlyingType {
        r_lgl_na()
    }

    fn convert_value(x: &Self::UnderlyingType) -> Self::Type {
        *x == 1
    }

    fn format_one(&self, x: Self::Type, _option: Option<&FormatOptions>) -> String {
        if x {
            String::from("TRUE")
        } else {
            String::from("FALSE")
        }
    }
}
