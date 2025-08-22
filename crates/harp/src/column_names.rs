use anyhow::anyhow;
use libr::*;

use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::utils::*;
use crate::vector::Vector;
use crate::CharacterVector;
use crate::RObject;

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
        let names = if r_typeof(names) == STRSXP {
            Some(unsafe { CharacterVector::new_unchecked(names) })
        } else {
            None
        };
        Self { names }
    }

    pub fn from_data_frame(x: SEXP) -> crate::Result<Self> {
        if !r_is_data_frame(x) {
            return Err(crate::anyhow!("`x` must be a data frame."));
        }
        let Some(column_names) = RObject::view(x).get_attribute_names() else {
            return Err(crate::anyhow!("`x` must have column names."));
        };
        Ok(Self::new(column_names.sexp))
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
            return names.get_unchecked(index);
        }
        None
    }

    pub fn get(&self, index: isize) -> anyhow::Result<Option<String>> {
        if let Some(names) = &self.names {
            return names.get(index).map_err(|err| anyhow!("{err:?}"));
        }
        Ok(None)
    }
}
