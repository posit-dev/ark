use libr::*;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::utils::*;
use crate::vector::Vector;
use crate::CharacterVector;

/// Column names
///
/// Column names represent an optional character vector of names. This class is mostly
/// useful for ergonomics, since [ColumnNames::get_unchecked()] will propagate [None]
/// when used on a vector without column names.
pub struct ColumnNames {
    names: Option<CharacterVector>,
}

impl ColumnNames {
    pub fn new(names: SEXP) -> Self {
        unsafe {
            let names = if r_typeof(names) == STRSXP {
                Some(CharacterVector::new_unchecked(names))
            } else {
                None
            };
            Self { names }
        }
    }

    pub fn from_data_frame(x: SEXP) -> crate::Result<Self> {
        if !r_is_data_frame(x) {
            return Err(crate::anyhow!("`x` must be a data frame."));
        }
        Ok(Self::new(r_names(x)))
    }

    pub fn from_matrix(x: SEXP) -> crate::Result<Self> {
        if !r_is_matrix(x) {
            return Err(crate::anyhow!("`x` must be a matrix."));
        }
        let column_names = RFunction::from("colnames").add(x).call()?;
        Ok(Self::new(column_names.sexp))
    }

    pub fn get_unchecked(&self, index: isize) -> Option<String> {
        if let Some(names) = &self.names {
            if let Some(name) = names.get_unchecked(index) {
                if name.len() > 0 {
                    return Some(name);
                }
            }
        }
        None
    }
}
