//
// mod.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use libr::Rf_allocVector;
use libr::Rf_xlength;
use libr::SEXP;

use crate::error::Result;
use crate::utils::r_assert_capacity;
use crate::utils::r_assert_type;

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

pub mod formatted_vector;
pub mod names;

// Formatting options for character vectors
pub struct FormatOptions {
    // Wether to quote the strings or not (defaults to `true`)
    // If `true`, elements will be quoted during format so, eg: c("a", "b") becomes ("\"a\"", "\"b\"") in Rust
    // Currently, this option is meaningful only for a character vector and is ignored on other types
    pub quote: bool,
}

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
            false => Some(Self::convert_value(&x)),
        }
    }

    fn get(&self, index: isize) -> Result<Option<Self::Type>> {
        unsafe {
            r_assert_capacity(self.data(), index as usize)?;
        }
        Ok(self.get_unchecked(index))
    }

    // Better name?
    fn get_value(&self, index: isize) -> Result<Self::Type> {
        let value = self
            .get(index)?
            .ok_or(crate::error::Error::MissingValueError)?;
        Ok(value)
    }

    unsafe fn new(object: impl Into<SEXP>) -> Result<Self>
    where
        Self: Sized,
    {
        let object = object.into();
        r_assert_type(object, &[Self::SEXPTYPE])?;
        Ok(Self::new_unchecked(object))
    }

    unsafe fn with_length(size: usize) -> Self
    where
        Self: Sized,
    {
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

    fn format_one(&self, x: Self::Type, options: Option<&FormatOptions>) -> String;

    fn format_elt_unchecked(&self, index: isize, options: Option<&FormatOptions>) -> String {
        match self.get_unchecked(index) {
            Some(x) => self.format_one(x, options),
            None => String::from("NA"),
        }
    }
}
