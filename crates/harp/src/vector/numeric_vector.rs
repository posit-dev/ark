//
// numeric_vector.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use libr::R_IsNA;
use libr::R_xlen_t;
use libr::Rf_allocVector;
use libr::DATAPTR;
use libr::REALSXP;
use libr::REAL_ELT;
use libr::SEXP;

use crate::object::RObject;
use crate::vector::Vector;

#[harp_macros::vector]
pub struct NumericVector {
    object: RObject,
}

impl Vector for NumericVector {
    type Item = f64;
    type Type = f64;
    const SEXPTYPE: u32 = REALSXP;
    type UnderlyingType = f64;
    type CompareType = f64;

    unsafe fn new_unchecked(object: impl Into<SEXP>) -> Self {
        Self {
            object: RObject::new(object.into()),
        }
    }

    fn create<T>(data: T) -> Self
    where
        T: IntoIterator,
        <T as IntoIterator>::IntoIter: ExactSizeIterator,
        <T as IntoIterator>::Item: AsRef<Self::Item>,
    {
        unsafe {
            let it = data.into_iter();
            let count = it.len();

            let vector = Rf_allocVector(Self::SEXPTYPE, count as R_xlen_t);
            let dataptr = DATAPTR(vector) as *mut Self::Type;
            it.enumerate().for_each(|(index, value)| {
                *(dataptr.offset(index as isize)) = *value.as_ref();
            });

            Self::new_unchecked(vector)
        }
    }

    fn data(&self) -> SEXP {
        self.object.sexp
    }

    fn is_na(x: &Self::UnderlyingType) -> bool {
        unsafe { R_IsNA(*x) == 1 }
    }

    fn get_unchecked_elt(&self, index: isize) -> Self::UnderlyingType {
        unsafe { REAL_ELT(self.data(), index as R_xlen_t) }
    }

    fn convert_value(x: &Self::UnderlyingType) -> Self::Type {
        *x
    }

    fn format_one(&self, x: Self::Type) -> String {
        x.to_string()
    }
}

impl TryFrom<&NumericVector> for Vec<f64> {
    type Error = harp::Error;

    fn try_from(value: &NumericVector) -> harp::Result<Self> {
        super::try_vec_from_r_vector(value)
    }
}
