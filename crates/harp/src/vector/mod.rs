//
// mod.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use itertools::Itertools;
use itertools::FoldWhile::{Continue, Done};
use libR_sys::*;

use crate::error::Result;
use crate::utils::r_assert_capacity;
use crate::utils::r_assert_type;
use crate::with_vector;

pub mod character_vector;
pub use character_vector::CharacterVector;

pub mod factor;
pub use factor::Factor;

pub mod integer_vector;
pub use integer_vector::IntegerVector;

pub mod logical_vector;
pub use logical_vector::LogicalVector;

pub mod numeric_vector;
pub use numeric_vector::NumericVector;

pub mod complex_vector;
pub use complex_vector::ComplexVector;

pub mod raw_vector;
pub use raw_vector::RawVector;

pub trait Vector {
    type Type;
    type Item: ?Sized;
    const SEXPTYPE: u32;
    type UnderlyingType;
    type CompareType;

    unsafe fn new_unchecked(object: impl Into<SEXP>) -> Self;
    fn data(&self) -> SEXP;
    fn is_na(x: &Self::UnderlyingType) -> bool;
    fn get_unchecked_elt(&self, index: isize) -> Self::UnderlyingType;
    fn convert_value(x: &Self::UnderlyingType) -> Self::Type;

    fn get_unchecked(&self, index: isize) -> Option<Self::Type> {
        let x = self.get_unchecked_elt(index);
        match Self::is_na(&x) {
            true => None,
            false => Some(Self::convert_value(&x))
        }
    }

    fn get(&self, index: isize) -> Result<Option<Self::Type>> {
        unsafe {
            r_assert_capacity(self.data(), index as u32)?;
        }
        Ok(self.get_unchecked(index))
    }

    unsafe fn new(object: impl Into<SEXP>) -> Result<Self> where Self: Sized {
        let object = object.into();
        r_assert_type(object, &[Self::SEXPTYPE])?;
        Ok(Self::new_unchecked(object))
    }

    unsafe fn with_length(size: usize) -> Self where Self: Sized {
        let data = Rf_allocVector(Self::SEXPTYPE, size as isize);
        Self::new_unchecked(data)
    }

    unsafe fn create<T>(data: T) -> Self
    where
        T: IntoIterator,
        <T as IntoIterator>::IntoIter: ExactSizeIterator,
        <T as IntoIterator>::Item: AsRef<Self::Item>;

    unsafe fn len(&self) -> usize {
        Rf_xlength(self.data()) as usize
    }

    fn format_one(&self, x: Self::Type) -> String;

}

pub struct Collapse {
    pub result: String,
    pub truncated: bool,
}

pub fn collapse(vector: SEXP, sep: &str, max: usize, quote: &str) -> Result<Collapse> {
    with_vector!(vector, |v| {
        let mut first = true;
        let formatted = v.iter().fold_while(String::from(""), |mut acc, x| {
            let added = format!("{}{}{}{}",
                if first {
                    first = false;
                    ""
                } else {
                    sep
                },
                quote,
                match x {
                    Some(x) => v.format_one(x),
                    None    => String::from("NA")
                },
                quote
            );
            acc.push_str(&added);
            if max > 0 && acc.len() > max {
                Done(acc)
            } else {
                Continue(acc)
            }
        });
        match formatted {
            Done(result) => Collapse{result, truncated: false},
            Continue(result) => Collapse{result, truncated: true}
        }
    })
}