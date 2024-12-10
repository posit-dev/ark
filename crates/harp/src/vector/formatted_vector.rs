//
// formatted_vector.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use anyhow::anyhow;
use libr::CPLXSXP;
use libr::INTSXP;
use libr::LGLSXP;
use libr::RAWSXP;
use libr::REALSXP;
use libr::STRSXP;

use crate::r_format_vec;
use crate::r_is_object;
use crate::r_length;
use crate::r_subset_vec;
use crate::table_info;
use crate::utils::r_assert_type;
use crate::utils::r_typeof;
use crate::vector::CharacterVector;
use crate::vector::ComplexVector;
use crate::vector::FormatOptions;
use crate::vector::IntegerVector;
use crate::vector::LogicalVector;
use crate::vector::NumericVector;
use crate::vector::RawVector;
use crate::vector::Vector;
use crate::RObject;

impl Default for FormatOptions {
    fn default() -> Self {
        Self { quote: true }
    }
}

pub struct FormattedVector {
    vector: RObject,
}

impl FormattedVector {
    pub fn new(vector: RObject) -> anyhow::Result<Self> {
        r_assert_type(vector.sexp, &[
            RAWSXP, LGLSXP, INTSXP, REALSXP, STRSXP, CPLXSXP,
        ])?;
        Ok(Self { vector })
    }

    /// Returns an iterator for the vector.
    /// Performance: for S3 objects this will cause the iterator to
    /// format the entire vector before starting the iteration.
    pub fn iter(&self) -> anyhow::Result<FormattedVectorIter> {
        FormattedVectorIter::new_unchecked(self.vector.clone(), None)
    }

    /// Returns an iterator over the first `n` elements of a vector.
    /// Should be used when the vector is potentially large and you won't need to
    /// iterate over the entire vector.
    pub fn iter_take(&self, n: usize) -> anyhow::Result<FormattedVectorIter> {
        // The iterators for atomic values and factors are lazy and don't need any special
        // treatment.
        let length = r_length(self.vector.sexp);
        let n = n.min(length as usize);

        FormattedVectorIter::new_unchecked(self.vector.clone(), Some(Box::new(0..n as i64)))
    }

    /// Formats a single element of a vector
    pub fn format_elt(&self, index: isize) -> anyhow::Result<String> {
        // Check if the index is allowed
        let length = r_length(self.vector.sexp);

        if index < 0 || index >= length as isize {
            return Err(anyhow!("Index out of bounds"));
        }

        let indices = vec![index as i64].into_iter();
        let result: Vec<String> =
            FormattedVectorIter::new_unchecked(self.vector.clone(), Some(Box::new(indices)))?
                .collect();

        if result.len() != 1 {
            return Err(anyhow!("Unexpected error"));
        }

        Ok(result[0].clone())
    }

    /// Returns an iterator over a column of a matrix.
    /// Subset a vector and return an iterator for the selected column.
    pub fn column_iter(&self, column: isize) -> anyhow::Result<FormattedVectorIter> {
        let indices = self.column_iter_indices(column)?;
        FormattedVectorIter::new_unchecked(self.vector.clone(), Some(Box::new(indices)))
    }

    /// Returns an iterator over the first `n` elements of a column of a matrix.
    pub fn column_iter_n(&self, column: isize, n: usize) -> anyhow::Result<FormattedVectorIter> {
        let indices = self.column_iter_indices(column)?.take(n);
        FormattedVectorIter::new_unchecked(self.vector.clone(), Some(Box::new(indices)))
    }

    fn column_iter_indices(&self, column: isize) -> anyhow::Result<std::ops::Range<i64>> {
        let dim = table_info(self.vector.sexp)
            .ok_or(anyhow!("Not a mtrix"))?
            .dims;

        let start = column as i64 * dim.num_rows as i64;
        let end = start + dim.num_rows as i64;
        Ok(start..end)
    }
}

enum AtomicVector {
    Raw(RawVector),
    Logical(LogicalVector),
    Integer(IntegerVector),
    Numeric(NumericVector),
    Character(CharacterVector),
    Complex(ComplexVector),
}

impl AtomicVector {
    fn new(vector: RObject) -> anyhow::Result<Self> {
        let vector = match r_typeof(vector.sexp) {
            RAWSXP => AtomicVector::Raw(unsafe { RawVector::new_unchecked(vector.sexp) }),
            LGLSXP => AtomicVector::Logical(unsafe { LogicalVector::new_unchecked(vector.sexp) }),
            INTSXP => AtomicVector::Integer(unsafe { IntegerVector::new_unchecked(vector.sexp) }),
            REALSXP => AtomicVector::Numeric(unsafe { NumericVector::new_unchecked(vector.sexp) }),
            STRSXP => {
                AtomicVector::Character(unsafe { CharacterVector::new_unchecked(vector.sexp) })
            },
            CPLXSXP => AtomicVector::Complex(unsafe { ComplexVector::new_unchecked(vector.sexp) }),
            _ => {
                return Err(anyhow!("Unsupported type"));
            },
        };
        Ok(vector)
    }

    fn format_element(&self, index: isize) -> String {
        // We always use the default options for now as this is only used for the variables pane,
        // we might want to change that in the future.
        let options = FormatOptions::default();
        match self {
            AtomicVector::Raw(v) => v.format_elt_unchecked(index, Some(&options)),
            AtomicVector::Logical(v) => v.format_elt_unchecked(index, Some(&options)),
            AtomicVector::Integer(v) => v.format_elt_unchecked(index, Some(&options)),
            AtomicVector::Numeric(v) => v.format_elt_unchecked(index, Some(&options)),
            AtomicVector::Character(v) => v.format_elt_unchecked(index, Some(&options)),
            AtomicVector::Complex(v) => v.format_elt_unchecked(index, Some(&options)),
        }
    }

    fn len(&self) -> usize {
        unsafe {
            match self {
                AtomicVector::Raw(v) => v.len(),
                AtomicVector::Logical(v) => v.len(),
                AtomicVector::Integer(v) => v.len(),
                AtomicVector::Numeric(v) => v.len(),
                AtomicVector::Character(v) => v.len(),
                AtomicVector::Complex(v) => v.len(),
            }
        }
    }
}

pub struct FormattedVectorIter {
    vector: AtomicVector,
    indices: Box<dyn Iterator<Item = i64>>,
}

impl FormattedVectorIter {
    /// Creates a new iterator over the formatted elements of a vector.
    /// If `indices` is `None`, the iterator will iterate over all elements of the vector.
    /// If `indices` is `Some`, the iterator will iterate over the elements at the specified indices.
    /// SAFETY: The caller must make sure that indices are valid and in bounds. Iteration may panic
    /// if the indices are out of bounds.
    fn new_unchecked(
        vector: RObject,
        indices: Option<Box<dyn Iterator<Item = i64>>>,
    ) -> anyhow::Result<Self> {
        // For atomic vectors we just create the iterator directly
        if !r_is_object(vector.sexp) {
            return Self::from_atomic(AtomicVector::new(vector)?, indices);
        }

        // For objects we need to format the vector before iterating. However, we can't
        // format the entire vector at once because it might be too large. Instead, we
        // subset the vector prior to formatting.
        let subset = match indices {
            None => vector,
            Some(indices) => {
                let indices = indices.collect::<Vec<i64>>();
                RObject::from(r_subset_vec(vector.sexp, indices)?)
            },
        };
        let formatted = RObject::from(r_format_vec(subset.sexp)?);

        // We already formatted the selected subset, so we can create an iterator over `None`
        // indices, ie, over all elements.
        Self::from_atomic(AtomicVector::new(formatted)?, None)
    }

    fn from_atomic(
        vector: AtomicVector,
        indices: Option<Box<dyn Iterator<Item = i64>>>,
    ) -> anyhow::Result<Self> {
        let indices = match indices {
            Some(indices) => indices,
            None => {
                let len = vector.len();
                Box::new(0..len as i64)
            },
        };

        return Ok(Self { vector, indices });
    }
}

impl Iterator for FormattedVectorIter {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(index) = self.indices.next() {
            Some(self.vector.format_element(index as isize))
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;

    use crate::environment::Environment;
    use crate::eval::parse_eval0;
    use crate::fixtures::r_task;
    use crate::modules::HARP_ENV;
    use crate::vector::formatted_vector::FormattedVector;
    use crate::RObject;

    #[test]
    fn test_unconforming_format_method() {
        // Test that we recover from unconforming `format()` methods
        r_task(|| unsafe {
            let exp = String::from("\"1\" \"2\"");

            // From src/modules/format.R
            let objs =
                Environment::new(parse_eval0("init_test_format()", HARP_ENV.unwrap()).unwrap());

            // Unconforming dims (posit-dev/positron#1862)
            let x = FormattedVector::new(RObject::from(objs.find("unconforming_dims").unwrap()))
                .unwrap();
            let out = x.column_iter(0).unwrap().join(" ");
            assert_eq!(out, exp);

            // Unconforming length
            let x = FormattedVector::new(RObject::from(objs.find("unconforming_length").unwrap()))
                .unwrap();
            let out = x.iter().unwrap().join(" ");
            assert_eq!(out, exp);

            // Unconforming type
            let x = FormattedVector::new(RObject::from(objs.find("unconforming_type").unwrap()))
                .unwrap();
            let out = x.iter().unwrap().join(" ");
            assert_eq!(out, exp);
        })
    }

    #[test]
    fn test_na_not_quoted() {
        r_task(|| {
            let x = harp::parse_eval_base(r#"c("1", "2", '"a"', "NA", NA_character_)"#).unwrap();

            let formatted = FormattedVector::new(x).unwrap();

            // NA is always unquoted regardless of the quote option
            let out = formatted.iter().unwrap().join(" ");
            assert_eq!(out, String::from(r#""1" "2" "\"a\"" "NA" NA"#));
        })
    }
}
