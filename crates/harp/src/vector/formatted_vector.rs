//
// formatted_vector.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//
use libR_sys::*;

use crate::error::Error;
use crate::exec::RFunction;
use crate::exec::RFunctionExt;
use crate::utils::r_assert_type;
use crate::utils::r_inherits;
use crate::utils::r_is_null;
use crate::utils::r_typeof;
use crate::vector::CharacterVector;
use crate::vector::ComplexVector;
use crate::vector::Factor;
use crate::vector::IntegerVector;
use crate::vector::LogicalVector;
use crate::vector::NumericVector;
use crate::vector::RawVector;
use crate::vector::Vector;

pub enum FormattedVector {
    // simple
    Raw { vector: RawVector },
    Logical { vector: LogicalVector },
    Integer { vector: IntegerVector },
    Numeric { vector: NumericVector },
    Character { vector: CharacterVector },
    Complex { vector: ComplexVector },
    // special
    Factor { vector: Factor },
    FormattedVector { vector: CharacterVector },
}

impl FormattedVector {
    pub fn new(vector: SEXP) -> Result<Self, Error> {
        unsafe {
            let class = Rf_getAttrib(vector, R_ClassSymbol);
            if r_is_null(class) {
                match r_typeof(vector) {
                    RAWSXP => Ok(Self::Raw {
                        vector: RawVector::new_unchecked(vector),
                    }),
                    LGLSXP => Ok(Self::Logical {
                        vector: LogicalVector::new_unchecked(vector),
                    }),
                    INTSXP => Ok(Self::Integer {
                        vector: IntegerVector::new_unchecked(vector),
                    }),
                    REALSXP => Ok(Self::Numeric {
                        vector: NumericVector::new_unchecked(vector),
                    }),
                    STRSXP => Ok(Self::Character {
                        vector: CharacterVector::new_unchecked(vector),
                    }),
                    CPLXSXP => Ok(Self::Complex {
                        vector: ComplexVector::new_unchecked(vector),
                    }),

                    _ => Err(Error::UnexpectedType(r_typeof(vector), vec![
                        RAWSXP, LGLSXP, INTSXP, REALSXP, STRSXP, CPLXSXP,
                    ])),
                }
            } else {
                if r_inherits(vector, "factor") {
                    Ok(Self::Factor {
                        vector: Factor::new_unchecked(vector),
                    })
                } else {
                    let formatted = RFunction::new("base", "format").add(vector).call()?;
                    r_assert_type(*formatted, &[STRSXP])?;
                    Ok(Self::FormattedVector {
                        vector: CharacterVector::new_unchecked(formatted),
                    })
                }
            }
        }
    }

    pub fn get_unchecked(&self, index: isize) -> String {
        match self {
            FormattedVector::Raw { vector } => vector.format_elt_unchecked(index),
            FormattedVector::Logical { vector } => vector.format_elt_unchecked(index),
            FormattedVector::Integer { vector } => vector.format_elt_unchecked(index),
            FormattedVector::Numeric { vector } => vector.format_elt_unchecked(index),
            FormattedVector::Character { vector } => vector.format_elt_unchecked(index),
            FormattedVector::Complex { vector } => vector.format_elt_unchecked(index),
            FormattedVector::Factor { vector } => vector.format_elt_unchecked(index),
            FormattedVector::FormattedVector { vector } => vector.format_elt_unchecked(index),
        }
    }
}
