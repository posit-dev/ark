//
// integer_vector.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use libr::R_NaInt;
use libr::R_xlen_t;
use libr::Rf_allocVector;
use libr::DATAPTR;
use libr::INTSXP;
use libr::SEXP;

use crate::object::RObject;
use crate::r_int_na;
use crate::try_int_get;
use crate::vector::FormatOptions;
use crate::vector::Vector;

#[harp_macros::vector]
pub struct IntegerVector {
    object: RObject,
}

impl Vector for IntegerVector {
    type Item = i32;
    type Type = i32;
    const SEXPTYPE: u32 = INTSXP;
    type UnderlyingType = i32;
    type CompareType = i32;

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
        try_int_get(self.data(), R_xlen_t::from(index))
    }

    fn error_elt() -> Self::UnderlyingType {
        r_int_na()
    }

    fn convert_value(x: &Self::UnderlyingType) -> Self::Type {
        *x
    }

    fn format_one(&self, x: Self::Type, _option: Option<&FormatOptions>) -> String {
        x.to_string()
    }
}
