//
// formatted_vector.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//
use libr::R_ClassSymbol;
use libr::R_DimSymbol;
use libr::Rf_getAttrib;
use libr::Rf_xlength;
use libr::CPLXSXP;
use libr::INTSXP;
use libr::LGLSXP;
use libr::RAWSXP;
use libr::REALSXP;
use libr::SEXP;
use libr::STRSXP;

use crate::error::Error;
use crate::error::Result;
use crate::r_format_vec;
use crate::utils::r_assert_type;
use crate::utils::r_inherits;
use crate::utils::r_is_null;
use crate::utils::r_typeof;
use crate::vector::CharacterVector;
use crate::vector::ComplexVector;
use crate::vector::Factor;
use crate::vector::FormatOptions;
use crate::vector::IntegerVector;
use crate::vector::LogicalVector;
use crate::vector::NumericVector;
use crate::vector::RawVector;
use crate::vector::Vector;
pub enum FormattedVector {
    // simple
    Raw {
        vector: RawVector,
    },
    Logical {
        vector: LogicalVector,
    },
    Integer {
        vector: IntegerVector,
    },
    Numeric {
        vector: NumericVector,
    },
    Character {
        vector: CharacterVector,
        options: FormatOptions,
    },
    Complex {
        vector: ComplexVector,
    },
    // special
    Factor {
        vector: Factor,
    },
    FormattedVector {
        vector: CharacterVector,
        options: FormatOptions,
    },
}

// Formatting options for vectors
#[derive(Default)]
pub struct FormattedVectorOptions {
    // Formatting options for character vectors
    pub character: FormatOptions,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self { quote: true }
    }
}

impl FormattedVector {
    pub fn new(vector: SEXP) -> Result<Self> {
        Self::new_with_options(vector, FormattedVectorOptions::default())
    }

    pub fn new_with_options(
        vector: SEXP,
        formatting_options: FormattedVectorOptions,
    ) -> Result<Self> {
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
                        options: formatting_options.character,
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
                    let formatted = r_format_vec(vector)?;

                    r_assert_type(formatted, &[STRSXP])?;
                    Ok(Self::FormattedVector {
                        vector: CharacterVector::new_unchecked(formatted),
                        options: formatting_options.character,
                    })
                }
            }
        }
    }

    pub fn get_unchecked(&self, index: isize) -> String {
        match self {
            FormattedVector::Raw { vector } => vector.format_elt_unchecked(index, None),
            FormattedVector::Logical { vector } => vector.format_elt_unchecked(index, None),
            FormattedVector::Integer { vector } => vector.format_elt_unchecked(index, None),
            FormattedVector::Numeric { vector } => vector.format_elt_unchecked(index, None),
            FormattedVector::Character { vector, options } => {
                vector.format_elt_unchecked(index, Some(options))
            },
            FormattedVector::Complex { vector } => vector.format_elt_unchecked(index, None),
            FormattedVector::Factor { vector } => vector.format_elt_unchecked(index, None),
            FormattedVector::FormattedVector { vector, options } => {
                vector.format_elt_unchecked(index, Some(options))
            },
        }
    }

    pub fn len(&self) -> isize {
        unsafe { Rf_xlength(self.data()) }
    }

    pub fn data(&self) -> SEXP {
        match self {
            FormattedVector::Raw { vector } => vector.data(),
            FormattedVector::Logical { vector } => vector.data(),
            FormattedVector::Integer { vector } => vector.data(),
            FormattedVector::Numeric { vector } => vector.data(),
            FormattedVector::Character { vector, options: _ } => vector.data(),
            FormattedVector::Complex { vector } => vector.data(),
            FormattedVector::Factor { vector } => vector.data(),
            FormattedVector::FormattedVector { vector, options: _ } => vector.data(),
        }
    }
}

pub struct FormattedVectorIter<'a> {
    formatted: &'a FormattedVector,
    index: isize,
    size: isize,
}

impl<'a> FormattedVectorIter<'a> {
    pub fn new(formatted: &'a FormattedVector) -> Self {
        Self {
            formatted,
            index: 0,
            size: formatted.len(),
        }
    }
}

impl<'a> Iterator for FormattedVectorIter<'a> {
    type Item = String;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index == self.size {
            None
        } else {
            let out = Some(self.formatted.get_unchecked(self.index));
            self.index = self.index + 1;
            out
        }
    }
}

impl FormattedVector {
    pub fn iter(&self) -> FormattedVectorIter {
        FormattedVectorIter::new(self)
    }

    pub fn column_iter(&self, column: isize) -> FormattedVectorIter {
        unsafe {
            let object = self.data();
            let dim = IntegerVector::new(Rf_getAttrib(object, R_DimSymbol)).unwrap();
            let n_row = dim.get_unchecked(0).unwrap() as isize;

            let index = column * n_row;

            FormattedVectorIter {
                formatted: self,
                index,
                size: index + n_row,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use itertools::Itertools;
    use libr::STRSXP;

    use crate::environment::Environment;
    use crate::eval::parse_eval0;
    use crate::modules::HARP_ENV;
    use crate::r_assert_type;
    use crate::test::r_task;
    use crate::vector::formatted_vector::FormattedVector;
    use crate::vector::formatted_vector::FormattedVectorOptions;
    use crate::vector::FormatOptions;

    #[test]
    fn test_unconforming_format_method() {
        // Test that we recover from unconforming `format()` methods
        r_task(|| unsafe {
            let exp = String::from("\"1\" \"2\"");

            // From src/modules/format.R
            let objs =
                Environment::new(parse_eval0("init_test_format()", HARP_ENV.unwrap()).unwrap());

            // Unconforming dims (posit-dev/positron#1862)
            let x = FormattedVector::new(objs.find("unconforming_dims").unwrap()).unwrap();
            let out = x.column_iter(0).join(" ");
            assert_eq!(out, exp);

            // Unconforming length
            let x = FormattedVector::new(objs.find("unconforming_length").unwrap()).unwrap();
            let out = x.iter().join(" ");
            assert_eq!(out, exp);

            // Unconforming type
            let x = FormattedVector::new(objs.find("unconforming_type").unwrap()).unwrap();
            let out = x.iter().join(" ");
            assert_eq!(out, exp);
        })
    }

    #[test]
    fn test_formatting_option() {
        r_task(|| {
            let x = harp::parse_eval_base(r#"c("1", "2", '"a"', "NA", NA_character_)"#).unwrap();
            r_assert_type(x.sexp, &[STRSXP]).unwrap();

            let formatted = FormattedVector::new_with_options(x.sexp, FormattedVectorOptions {
                character: FormatOptions { quote: false },
            })
            .unwrap();

            let out = formatted.iter().join(" ");
            assert_eq!(out, String::from(r#"1 2 "a" NA NA"#));

            let formatted = FormattedVector::new_with_options(x.sexp, FormattedVectorOptions {
                character: FormatOptions { quote: true },
            })
            .unwrap();

            // NA is always unquoted regardless of the quote option
            let out = formatted.iter().join(" ");
            assert_eq!(out, String::from(r#""1" "2" "\"a\"" "NA" NA"#));
        })
    }
}
