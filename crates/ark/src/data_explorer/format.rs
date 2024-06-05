//
// format.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use amalthea::comm::data_explorer_comm::ColumnValue;
use amalthea::comm::data_explorer_comm::FormatOptions;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::r_dbl_is_finite;
use harp::object::r_dbl_is_nan;
use harp::object::r_length;
use harp::object::r_list_get;
use harp::object::RObject;
use harp::utils::r_classes;
use harp::utils::r_format;
use harp::utils::r_is_null;
use harp::utils::r_typeof;
use harp::vector::CharacterVector;
use harp::vector::ComplexVector;
use harp::vector::IntegerVector;
use harp::vector::LogicalVector;
use harp::vector::NumericVector;
use harp::vector::Vector;
use libr::Rf_xlength;
use libr::SEXP;
use libr::*;

use crate::modules::ARK_ENVS;

// Format a column of data for display in the data explorer.
pub fn format_column(x: SEXP, format_options: &FormatOptions) -> anyhow::Result<Vec<ColumnValue>> {
    let formatted = format_values(x, format_options)?;
    let special_value_codes = special_values(x);

    let output = formatted
        .into_iter()
        .zip(special_value_codes.into_iter())
        .map(|(val, code)| match code {
            SpecialValueTypes::NotSpecial => ColumnValue::FormattedValue(val),
            _ => ColumnValue::SpecialValueCode(code.into()),
        })
        .collect();

    Ok(output)
}

fn format_values(x: SEXP, format_options: &FormatOptions) -> anyhow::Result<Vec<String>> {
    // If the object has classes we dispatch to the `format` method
    if let Some(_) = r_classes(x) {
        return Ok(RObject::from(r_format(x)?).try_into()?);
    }

    let formatted: Vec<String> = match r_typeof(x) {
        INTSXP => RFunction::from("format_integer")
            .add(x)
            .param("thousands_sep", format_options.thousands_sep.clone())
            .call_in(ARK_ENVS.positron_ns)?
            .try_into()?,
        REALSXP => RFunction::from("format_real")
            .add(x)
            .param("thousands_sep", format_options.thousands_sep.clone())
            .param("large_num_digits", format_options.large_num_digits as i32)
            .param("small_num_digits", format_options.small_num_digits as i32)
            .param(
                "max_integral_digits",
                format_options.max_integral_digits as i32,
            )
            .call_in(ARK_ENVS.positron_ns)?
            .try_into()?,
        // For list columns we do something similar to tibbles, ie
        // show the element <class [length]>.
        VECSXP => RFunction::new("", "format_list_column")
            .add(x)
            .call_in(ARK_ENVS.positron_ns)?
            .try_into()?,
        // For all other values we rely on base R formatting
        _ => RObject::from(r_format(x)?).try_into()?,
    };

    Ok(formatted)
}

#[derive(Clone)]
enum SpecialValueTypes {
    NotSpecial,
    NULL,
    NA,
    NaN,
    Inf,
    NegInf,
}

// Find the special code values mapping to integer here:
// https://github.com/posit-dev/positron/blob/46eb4dc0b071984be0f083c7836d74a19ef1509f/src/vs/workbench/services/positronDataExplorer/common/dataExplorerCache.ts#L59-L60
impl Into<i64> for SpecialValueTypes {
    fn into(self) -> i64 {
        match self {
            SpecialValueTypes::NotSpecial => -1,
            SpecialValueTypes::NULL => 0,
            SpecialValueTypes::NA => 1,
            SpecialValueTypes::NaN => 2,
            SpecialValueTypes::Inf => 10,
            SpecialValueTypes::NegInf => 11,
        }
    }
}

// Returns an iterator that checks for special values in a vector.
fn special_values(object: SEXP) -> Vec<SpecialValueTypes> {
    match r_typeof(object) {
        REALSXP => {
            let data = unsafe { NumericVector::new_unchecked(object) };
            data.iter()
                .map(|x| match x {
                    Some(v) => {
                        if r_dbl_is_nan(v) {
                            SpecialValueTypes::NaN
                        } else if !r_dbl_is_finite(v) {
                            if v < 0.0 {
                                SpecialValueTypes::NegInf
                            } else {
                                SpecialValueTypes::Inf
                            }
                        } else {
                            SpecialValueTypes::NotSpecial
                        }
                    },
                    None => SpecialValueTypes::NA,
                })
                .collect()
        },
        STRSXP => {
            let data = unsafe { CharacterVector::new_unchecked(object) };
            data.iter()
                .map(|x| match x {
                    Some(_) => SpecialValueTypes::NotSpecial,
                    None => SpecialValueTypes::NA,
                })
                .collect()
        },
        INTSXP => {
            let data = unsafe { IntegerVector::new_unchecked(object) };
            data.iter()
                .map(|x| match x {
                    Some(_) => SpecialValueTypes::NotSpecial,
                    None => SpecialValueTypes::NA,
                })
                .collect()
        },
        LGLSXP => {
            let data = unsafe { LogicalVector::new_unchecked(object) };
            data.iter()
                .map(|x| match x {
                    Some(_) => SpecialValueTypes::NotSpecial,
                    None => SpecialValueTypes::NA,
                })
                .collect()
        },
        CPLXSXP => {
            let data = unsafe { ComplexVector::new_unchecked(object) };
            data.iter()
                .map(|x| match x {
                    Some(_) => SpecialValueTypes::NotSpecial,
                    None => SpecialValueTypes::NA,
                })
                .collect()
        },
        VECSXP => (0..r_length(object))
            .map(|i| {
                if r_is_null(r_list_get(object, i)) {
                    SpecialValueTypes::NULL
                } else {
                    SpecialValueTypes::NotSpecial
                }
            })
            .collect(),
        _ => vec![SpecialValueTypes::NotSpecial; unsafe { Rf_xlength(object) as usize }],
    }
}
