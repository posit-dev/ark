//
// logical_vector.rs
//
// Copyright (C) 2022-2024 Posit Software, PBC. All rights reserved.
//
//

use libr::R_NaInt;
use libr::R_xlen_t;
use libr::Rf_allocVector;
use libr::DATAPTR;
use libr::LGLSXP;
use libr::LOGICAL_ELT;
use libr::SEXP;

use crate::object::RObject;
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
        unsafe { *x == R_NaInt }
    }

    fn get_unchecked_elt(&self, index: isize) -> Self::UnderlyingType {
        unsafe { LOGICAL_ELT(self.data(), index as R_xlen_t) }
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

impl TryFrom<&LogicalVector> for Vec<bool> {
    type Error = harp::Error;

    fn try_from(value: &LogicalVector) -> harp::Result<Self> {
        super::try_vec_from_r_vector(value)
    }
}
