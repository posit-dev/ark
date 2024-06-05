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
use harp::r_null;
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
use libr::SEXP;
use libr::*;

use crate::modules::ARK_ENVS;

pub fn format_column(x: SEXP, format_options: &FormatOptions) -> Vec<ColumnValue> {
    format_values(x, format_options).unwrap_or(unknown_format(x))
}

fn unknown_format(x: SEXP) -> Vec<ColumnValue> {
    vec![ColumnValue::FormattedValue("????".to_string()); r_length(x) as usize]
}

// Format a column of data for display in the data explorer.
fn format_values(x: SEXP, format_options: &FormatOptions) -> anyhow::Result<Vec<ColumnValue>> {
    if let Some(_) = r_classes(x) {
        let formatted: Vec<String> = RObject::from(r_format(x)?).try_into()?;
        return Ok(formatted
            .into_iter()
            .map(ColumnValue::FormattedValue)
            .collect());
    }

    match r_typeof(x) {
        REALSXP => Ok(format_dbl(x, format_options)),
        INTSXP => Ok(format_int(x, format_options)),
        STRSXP => Ok(format_chr(x)),
        LGLSXP => Ok(format_lgl(x)),
        CPLXSXP => Ok(format_cpl(x)),
        VECSXP => Ok(format_vec(x)),
        _ => Err(anyhow::anyhow!("Unsupported column type")),
    }
}

fn format_vec(x: SEXP) -> Vec<ColumnValue> {
    let len = r_length(x);
    let mut output = Vec::<ColumnValue>::with_capacity(len as usize);

    for i in 0..len {
        let elt = r_list_get(x, i);
        let formatted = if r_is_null(elt) {
            SpecialValueTypes::NULL.into()
        } else {
            ColumnValue::FormattedValue(format_vec_elt(elt))
        };
        output.push(formatted);
    }

    output
}

fn format_vec_elt(x: SEXP) -> String {
    // We don't use `r_classes` because we want to see, eg 'numeric' for
    // numeric vectors, not an empty value.
    let class: Vec<String> = RFunction::new("base", "class")
        .add(x)
        .call_in(ARK_ENVS.positron_ns)
        // failling to evaluate classes will return NULL
        .unwrap_or(RObject::from(r_null()))
        .try_into()
        // if we fail to get the class we will show ????
        .unwrap_or(vec![]);

    let class_str = if class.is_empty() {
        "????:".to_string()
    } else {
        class[0].clone()
    };

    // we call `dim` and not `r_dim()` because we want to see the values
    // from the dispatched method.
    let dims: Vec<i32> = RFunction::from("dim")
        .add(x)
        .call_in(ARK_ENVS.positron_ns)
        // if we fail to get the dims we will show the length of the object instead
        .unwrap_or(RObject::from(r_null()))
        .try_into()
        .unwrap_or(vec![]);

    let dim_str: String = if dims.is_empty() {
        r_length(x).to_string()
    } else {
        dims.iter()
            .map(i32::to_string)
            .collect::<Vec<String>>()
            .join(" x ")
    };

    format!("<{} [{}]>", class_str, dim_str)
}

fn format_cpl(x: SEXP) -> Vec<ColumnValue> {
    unsafe {
        ComplexVector::new_unchecked(x)
            .iter()
            .map(|x| match x {
                Some(v) => {
                    ColumnValue::FormattedValue(format!("{}+{}i", v.r.to_string(), v.i.to_string()))
                },
                None => SpecialValueTypes::NA.into(),
            })
            .collect()
    }
}

fn format_lgl(x: SEXP) -> Vec<ColumnValue> {
    unsafe {
        LogicalVector::new_unchecked(x)
            .iter()
            .map(|x| match x {
                Some(v) => match v {
                    true => ColumnValue::FormattedValue("TRUE".to_string()),
                    false => ColumnValue::FormattedValue("FALSE".to_string()),
                },
                None => SpecialValueTypes::NA.into(),
            })
            .collect()
    }
}

fn format_chr(x: SEXP) -> Vec<ColumnValue> {
    unsafe {
        CharacterVector::new_unchecked(x)
            .iter()
            .map(|x| match x {
                Some(v) => ColumnValue::FormattedValue(v),
                None => SpecialValueTypes::NA.into(),
            })
            .collect()
    }
}

fn format_int(x: SEXP, options: &FormatOptions) -> Vec<ColumnValue> {
    unsafe {
        IntegerVector::new_unchecked(x)
            .iter()
            .map(|x| format_int_elt(x, options))
            .collect()
    }
}

fn format_int_elt(x: Option<i32>, options: &FormatOptions) -> ColumnValue {
    match x {
        None => SpecialValueTypes::NA.into(),
        Some(v) => ColumnValue::FormattedValue(apply_thousands_sep(
            v.to_string(),
            options.thousands_sep.clone(),
        )),
    }
}

fn format_dbl(x: SEXP, options: &FormatOptions) -> Vec<ColumnValue> {
    unsafe {
        NumericVector::new_unchecked(x)
            .iter()
            .map(|x| format_dbl_elt(x, options))
            .collect()
    }
}

fn format_dbl_elt(x: Option<f64>, options: &FormatOptions) -> ColumnValue {
    match x {
        None => SpecialValueTypes::NA.into(),
        Some(v) => {
            if r_dbl_is_nan(v) {
                SpecialValueTypes::NaN.into()
            } else if r_dbl_is_finite(v) {
                // finite values that are not NaN nor NA
                format_dbl_value(v, options)
            } else if v > 0.0 {
                SpecialValueTypes::Inf.into()
            } else {
                SpecialValueTypes::NegInf.into()
            }
        },
    }
}

fn format_dbl_value(x: f64, options: &FormatOptions) -> ColumnValue {
    // The limit for large numbers before switching to scientific
    // notation
    let upper_threshold = f64::powf(10.0, options.max_integral_digits as f64);

    // The limit for small numbers before switching to scientific
    // notation
    let lower_threshold = f64::powf(10.0, -(options.small_num_digits as f64));

    let large_num_digits = options.large_num_digits as usize;
    let small_num_digits = options.small_num_digits as usize;

    let abs_x = x.abs();

    let formatted = if abs_x >= upper_threshold {
        // large numbers use scientific notation
        // rust makes 1e7 instead of 1e+7 which aligns baddly
        let v = format!("{:.large_num_digits$e}", x).replace("e", "e+");
        pad_exponent(v)
    } else if abs_x >= 1.0 {
        // this is considered medium numbers and they use a fixed amount of
        // digits after the decimal point
        apply_thousands_sep(
            format!("{:.large_num_digits$}", x),
            options.thousands_sep.clone(),
        )
    } else if abs_x >= lower_threshold {
        // small numbers but not that small are formatted with a different
        // amount of digits after the decimal point
        apply_thousands_sep(
            format!("{:.small_num_digits$}", x),
            options.thousands_sep.clone(),
        )
    } else if abs_x == 0.0 {
        // zero is special cased to behave like a medium number.
        format!("{:.large_num_digits$}", x)
    } else {
        // very small numbers use scientific notation
        let v = format!("{:.large_num_digits$e}", x);
        pad_exponent(v)
    };

    ColumnValue::FormattedValue(formatted)
}

fn apply_thousands_sep(x: String, sep: Option<String>) -> String {
    match sep {
        None => x,
        Some(sep) => {
            let mut formatted = String::new();

            // find the decimal point if any
            let decimal_point = x.find('.').unwrap_or(x.len());

            // now walk from the decimal point until walk the string
            // backwards adding the thousands separator
            let mut count: usize = 0;
            for (i, c) in x.chars().rev().enumerate() {
                // while before the decimal point, just copy the string
                if i < (x.len() - decimal_point) {
                    formatted.push(c);
                    continue;
                }

                // now start counting and add a `sep` every three
                if count % 3 == 0 && count != 0 {
                    formatted.push_str(&sep);
                }

                formatted.push(c);
                count += 1;
            }
            formatted.chars().rev().collect::<String>()
        },
    }
}

// exponents of the scientific notation should have a minimum of length 2
// to match the other implementations
// the string must have already been processed to include the e+ in positive values
fn pad_exponent(x: String) -> String {
    // find the exponent position
    let e_pos = x.find('e').unwrap();
    if (e_pos + 1 + 2) < x.len() {
        return x; // the exponent already have 2 digits
    }

    // add zeros to the exponent
    let mut formatted = x.clone();
    formatted.insert(e_pos + 2, '0');

    formatted
}

#[cfg(test)]
mod tests {
    use harp::environment::R_ENVS;
    use harp::eval::r_parse_eval0;

    use super::*;
    use crate::test::r_test;

    fn default_options() -> FormatOptions {
        FormatOptions {
            large_num_digits: 2,
            small_num_digits: 4,
            max_integral_digits: 7,
            thousands_sep: None,
        }
    }

    #[test]
    fn test_pad_exponents() {
        assert_eq!(pad_exponent("1.00e+111".to_string()), "1.00e+111");
        assert_eq!(pad_exponent("1.00e+11".to_string()), "1.00e+11");
        assert_eq!(pad_exponent("1.00e+01".to_string()), "1.00e+01");
        assert_eq!(pad_exponent("1.00e+1".to_string()), "1.00e+01");
        assert_eq!(pad_exponent("1.00e+00".to_string()), "1.00e+00");
        assert_eq!(pad_exponent("1.00e-01".to_string()), "1.00e-01");
        assert_eq!(pad_exponent("1.00e-1".to_string()), "1.00e-01");
        assert_eq!(pad_exponent("1.00e-00".to_string()), "1.00e-00");
    }

    #[test]
    fn test_thousands_sep() {
        assert_eq!(
            apply_thousands_sep("1000000".to_string(), Some(",".to_string())),
            "1,000,000"
        );
        assert_eq!(
            apply_thousands_sep("1000000.000".to_string(), Some(",".to_string())),
            "1,000,000.000"
        );
        assert_eq!(
            apply_thousands_sep("1.00".to_string(), Some(",".to_string())),
            "1.00"
        );
        assert_eq!(
            apply_thousands_sep("1000.00".to_string(), Some(",".to_string())),
            "1,000.00"
        );
        assert_eq!(
            apply_thousands_sep("100.00".to_string(), Some(",".to_string())),
            "100.00"
        );
        assert_eq!(
            apply_thousands_sep("1000000.00".to_string(), None),
            "1000000.00"
        );
    }

    #[test]
    fn test_real_formatting() {
        r_test(|| {
            // this test needs to match the Python equivalent in
            // https://github.com/posit-dev/positron/blob/5192792967b6778608d643b821e84ebb6d5f7025/extensions/positron-python/python_files/positron/positron_ipykernel/tests/test_data_explorer.py#L742-L743
            let assert_float_formatting = |options: FormatOptions, expected: Vec<ColumnValue>| {
                let testing_values = r_parse_eval0(
                    r#"c(
                                0,
                                1.0,
                                1.01,
                                1.012,
                                0.0123,
                                0.01234,
                                0.0001,
                                0.00001,
                                9999.123,
                                9999.999,
                                9999999,
                                10000000
                            )"#,
                    R_ENVS.global,
                )
                .unwrap();

                let formatted = format_column(testing_values.sexp, &options);
                assert_eq!(formatted, expected);
            };

            let options = FormatOptions {
                large_num_digits: 2,
                small_num_digits: 4,
                max_integral_digits: 7,
                thousands_sep: None,
            };
            let expected = vec![
                "0.00",
                "1.00",
                "1.01",
                "1.01",
                "0.0123",
                "0.0123",
                "0.0001",
                "1.00e-05",
                "9999.12",
                "10000.00",
                "9999999.00",
                "1.00e+07",
            ]
            .iter()
            .map(|x| ColumnValue::FormattedValue(x.to_string()))
            .collect::<Vec<ColumnValue>>();

            assert_float_formatting(options, expected);

            let options = FormatOptions {
                large_num_digits: 3,
                small_num_digits: 4,
                max_integral_digits: 7,
                thousands_sep: Some("_".to_string()),
            };

            let expected = vec![
                "0.000",
                "1.000",
                "1.010",
                "1.012",
                "0.0123",
                "0.0123",
                "0.0001",
                "1.000e-05",
                "9_999.123",
                "9_999.999",
                "9_999_999.000",
                "1.000e+07",
            ]
            .iter()
            .map(|x| ColumnValue::FormattedValue(x.to_string()))
            .collect::<Vec<ColumnValue>>();

            assert_float_formatting(options, expected);
        })
    }

    #[test]
    fn test_list_formatting() {
        r_test(|| {
            let data = r_parse_eval0("list(0, NULL)", R_ENVS.global).unwrap();
            let formatted = format_column(data.sexp, &default_options());
            assert_eq!(formatted, vec![
                ColumnValue::FormattedValue("<numeric [1]>".to_string()),
                SpecialValueTypes::NULL.into()
            ]);
        })
    }
}

#[derive(Clone)]
enum SpecialValueTypes {
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
            SpecialValueTypes::NULL => 0,
            SpecialValueTypes::NA => 1,
            SpecialValueTypes::NaN => 2,
            SpecialValueTypes::Inf => 10,
            SpecialValueTypes::NegInf => 11,
        }
    }
}

impl Into<ColumnValue> for SpecialValueTypes {
    fn into(self) -> ColumnValue {
        ColumnValue::SpecialValueCode(self.into())
    }
}
