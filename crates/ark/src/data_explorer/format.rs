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
use stdext::unwrap;

use crate::modules::ARK_ENVS;

const FALLBACK_FORMAT_STRING: &str = "????";

// Used by the get_data_values method to format columns for displaying in the grid.
pub fn format_column(x: SEXP, format_options: &FormatOptions) -> Vec<ColumnValue> {
    format(x, format_options)
        .into_iter()
        .map(Into::into)
        .collect()
}

// Used by the summary_profile method to format the summary statistics for display.
pub fn format_string(x: SEXP, format_options: &FormatOptions) -> Vec<String> {
    format(x, format_options)
        .into_iter()
        .map(Into::into)
        .collect()
}

fn format(x: SEXP, format_options: &FormatOptions) -> Vec<FormattedValue> {
    let mut formatted = format_values(x, format_options).unwrap_or(unknown_format(x));

    // Truncate the values if they are too long
    formatted.iter_mut().for_each(|v| {
        if let FormattedValue::Value(x) = v {
            truncate_inplace(x, format_options.max_value_length);
        }
    });

    formatted
}

// Truncating strings in Rust is more complicated that one would imagine.
// If you index using eg s[..6] that would take 6 bytes, which is not necessarily 6 characters.
// We need to iterate over the characters and truncate the string at the right character.
// See also https://doc.rust-lang.org/book/ch08-02-strings.html
fn truncate_inplace(s: &mut String, max_len: i64) {
    // Returns the indice of the nth character in the string if it exists
    let index = s.char_indices().nth(max_len as usize);

    // If an index is found, truncate the string to that index
    match index {
        // truncate is an in-place operation that takes the new max index as input
        // but looking the string as a sequence of bytes, not chars.
        Some((i, _)) => s.truncate(i),
        None => (),
    }
}

fn unknown_format(x: SEXP) -> Vec<FormattedValue> {
    vec![FormattedValue::Unkown; r_length(x) as usize]
}

// Format a column of data for display in the data explorer.
fn format_values(x: SEXP, format_options: &FormatOptions) -> anyhow::Result<Vec<FormattedValue>> {
    if let Some(_) = r_classes(x) {
        return Ok(format_object(x));
    }

    match r_typeof(x) {
        REALSXP => Ok(format_dbl(
            unsafe { NumericVector::new_unchecked(x) },
            format_options,
        )),
        INTSXP => Ok(format_int(
            unsafe { IntegerVector::new_unchecked(x) },
            format_options,
        )),
        STRSXP => Ok(format_chr(unsafe { CharacterVector::new_unchecked(x) })),
        LGLSXP => Ok(format_lgl(unsafe { LogicalVector::new_unchecked(x) })),
        CPLXSXP => Ok(format_cpl(unsafe { ComplexVector::new_unchecked(x) })),
        VECSXP => Ok(format_list(x)),
        _ => Err(anyhow::anyhow!("Unsupported column type")),
    }
}

fn format_object(x: SEXP) -> Vec<FormattedValue> {
    // We call r_format() to dispatch the format method
    let formatted: Vec<Option<String>> = match r_format(x) {
        Ok(fmt) => match RObject::from(fmt).try_into() {
            Ok(x) => x,
            Err(_) => return unknown_format(x),
        },
        Err(_) => return unknown_format(x),
    };

    let formatted = formatted.into_iter().map(|x| {
        match x {
            Some(v) => {
                // `base::format` defaults to using `trim=FALSE`
                // So it will add spaces to the end of the strings causing all elements of the vector
                // to have the same fixed width. We don't want this behavior in the data explorer,
                // We tried passing `trim=TRUE` but this is unfortunately not supported for eg. `factors`:
                // > format(factor(c("aaaa", "a")), trim = TRUE)
                // [1] "aaaa" "a   "
                //
                // So we will just trim the spaces manually, which is not ideal, but it's better than
                // having the values misaligned
                FormattedValue::Value(v.trim_matches(|x| x == ' ').to_string())
            },
            // In some cases `format()` will return `NA` for values it can't format instead of `"NA"`.
            // For example, with `format(as.POSIXct(c(NA)))`.
            None => FormattedValue::NA,
        }
    });

    // But we also want to show special value codes. We call `base::is.na()` to dispatch
    // the `is.na()` function and then replace those with `FormattedValues::NA`.
    let is_na = RFunction::from("is_na_checked")
        .add(x)
        .call_in(ARK_ENVS.positron_ns);

    let is_na = unwrap!(is_na, Err(_) => {
        // If we fail to evaluate `is.na()` we will just return the formatted values
        // as is.
        return formatted.collect();
    });

    unsafe { LogicalVector::new_unchecked(is_na.sexp) }
        .iter()
        .zip(formatted)
        .map(|(is_na, v)| {
            // We don't expect is.na to return NA's, but if it happens, we treat it as false
            // and return the formatted values as is.
            if is_na.unwrap_or(false) {
                FormattedValue::NA
            } else {
                v
            }
        })
        .collect()
}

fn format_list(x: SEXP) -> Vec<FormattedValue> {
    let len = r_length(x);
    let mut output = Vec::<FormattedValue>::with_capacity(len as usize);

    for i in 0..len {
        let elt = harp::list_get(x, i);
        let formatted = if r_is_null(elt) {
            FormattedValue::NULL
        } else {
            FormattedValue::Value(format_list_elt(elt))
        };
        output.push(formatted);
    }

    output
}

fn format_list_elt(x: SEXP) -> String {
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
        format!("{}:", FALLBACK_FORMAT_STRING)
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

fn format_cpl(x: ComplexVector) -> Vec<FormattedValue> {
    x.iter()
        .map(|x| match x {
            Some(v) => FormattedValue::Value(format!("{}+{}i", v.r, v.i)),
            None => FormattedValue::NA,
        })
        .collect()
}

fn format_lgl(x: LogicalVector) -> Vec<FormattedValue> {
    x.iter()
        .map(|x| match x {
            Some(v) => match v {
                true => FormattedValue::Value("TRUE".to_string()),
                false => FormattedValue::Value("FALSE".to_string()),
            },
            None => FormattedValue::NA,
        })
        .collect()
}

fn format_chr(x: CharacterVector) -> Vec<FormattedValue> {
    x.iter()
        .map(|x| match x {
            Some(v) => FormattedValue::Value(v),
            None => FormattedValue::NA,
        })
        .collect()
}

fn format_int(x: IntegerVector, options: &FormatOptions) -> Vec<FormattedValue> {
    x.iter().map(|x| format_int_elt(x, options)).collect()
}

fn format_int_elt(x: Option<i32>, options: &FormatOptions) -> FormattedValue {
    match x {
        None => FormattedValue::NA,
        Some(v) => FormattedValue::Value(apply_thousands_sep(
            v.to_string(),
            options.thousands_sep.clone(),
        )),
    }
}

fn format_dbl(x: NumericVector, options: &FormatOptions) -> Vec<FormattedValue> {
    x.iter().map(|x| format_dbl_elt(x, options)).collect()
}

fn format_dbl_elt(x: Option<f64>, options: &FormatOptions) -> FormattedValue {
    match x {
        None => FormattedValue::NA,
        Some(v) => {
            if r_dbl_is_nan(v) {
                FormattedValue::NaN
            } else if r_dbl_is_finite(v) {
                // finite values that are not NaN nor NA
                format_dbl_value(v, options)
            } else if v > 0.0 {
                FormattedValue::Inf
            } else {
                FormattedValue::NegInf
            }
        },
    }
}

fn format_dbl_value(x: f64, options: &FormatOptions) -> FormattedValue {
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

    FormattedValue::Value(formatted)
}

fn apply_thousands_sep(x: String, sep: Option<String>) -> String {
    match sep {
        None => x,
        Some(sep) => {
            let mut formatted = String::new();

            // Find the decimal point if any
            let decimal_point = x.find('.').unwrap_or(x.len());

            // Walk backwards on the string to add the thousands separator
            let mut count: usize = 0;
            for (i, c) in x.chars().rev().enumerate() {
                // Now walk backwards until we reach the decimal point.
                // After the point, start adding the thousands separator.
                if i < (x.len() - decimal_point) {
                    formatted.push(c);
                    continue;
                }

                // For negative numbers, break the iteration.
                // `continue` should have the same effect as `break` as there shouldn't exist
                // any character before `-`.
                // This avoids '-100' to be formatted as '-,100'.
                if c == '-' {
                    formatted.push(c);
                    continue;
                }

                // Add a `sep` every three characters
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
    let e_pos = match x.find('e') {
        Some(v) => v,
        None => return x, // if no e is found, return the string as is
    };

    // "1e-12" the e_pos (1) + 3 is < x.len() (5)
    // "1e-1"  the e_pos (1) + 3 is == x.len() (4)
    if (e_pos + 1 + 2) < x.len() {
        return x; // the exponent already have 2 digits
    }

    // add zeros to the exponent
    let mut formatted = x;
    formatted.insert(e_pos + 2, '0');

    formatted
}

// This type is only internally used with the intent of being easy to convert to
// ColumnValue or String when needed.
#[derive(Clone)]
enum FormattedValue {
    Unkown,
    NULL,
    NA,
    NaN,
    Inf,
    NegInf,
    Value(String),
}

// Find the special code values mapping to integer here:
// https://github.com/posit-dev/positron/blob/46eb4dc0b071984be0f083c7836d74a19ef1509f/src/vs/workbench/services/positronDataExplorer/common/dataExplorerCache.ts#L59-L60
impl Into<ColumnValue> for FormattedValue {
    fn into(self) -> ColumnValue {
        match self {
            FormattedValue::Unkown => ColumnValue::FormattedValue(self.into()),
            FormattedValue::NULL => ColumnValue::SpecialValueCode(0),
            FormattedValue::NA => ColumnValue::SpecialValueCode(1),
            FormattedValue::NaN => ColumnValue::SpecialValueCode(2),
            FormattedValue::Inf => ColumnValue::SpecialValueCode(10),
            FormattedValue::NegInf => ColumnValue::SpecialValueCode(11),
            FormattedValue::Value(v) => ColumnValue::FormattedValue(v),
        }
    }
}

impl Into<String> for FormattedValue {
    fn into(self) -> String {
        match self {
            FormattedValue::NULL => "NULL".to_string(),
            FormattedValue::NA => "NA".to_string(),
            FormattedValue::NaN => "NaN".to_string(),
            FormattedValue::Inf => "Inf".to_string(),
            FormattedValue::NegInf => "-Inf".to_string(),
            FormattedValue::Unkown => FALLBACK_FORMAT_STRING.to_string(),
            FormattedValue::Value(v) => v,
        }
    }
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
            thousands_sep: Some(",".to_string()),
            max_value_length: 100,
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
        assert_eq!(
            apply_thousands_sep("-100".to_string(), Some(",".to_string())),
            "-100"
        );
        assert_eq!(
            apply_thousands_sep("-100000".to_string(), Some(",".to_string())),
            "-100,000"
        );
        assert_eq!(
            apply_thousands_sep("-100000.00".to_string(), Some(",".to_string())),
            "-100,000.00"
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
                max_value_length: 100,
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
                max_value_length: 100,
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
    fn test_float_special_values() {
        r_test(|| {
            let data = r_parse_eval0(
                "c(NA_real_, NaN, Inf, -Inf, 0, 1, 1000000000, -1000000000)",
                R_ENVS.global,
            )
            .unwrap();
            let formatted = format_column(data.sexp, &default_options());
            assert_eq!(formatted, vec![
                FormattedValue::NA.into(),
                FormattedValue::NaN.into(),
                FormattedValue::Inf.into(),
                FormattedValue::NegInf.into(),
                ColumnValue::FormattedValue("0.00".to_string()),
                ColumnValue::FormattedValue("1.00".to_string()),
                ColumnValue::FormattedValue("1.00e+09".to_string()),
                ColumnValue::FormattedValue("-1.00e+09".to_string())
            ]);
        })
    }

    #[test]
    fn test_list_formatting() {
        r_test(|| {
            let data = r_parse_eval0("list(0, NULL, NA_real_)", R_ENVS.global).unwrap();
            let formatted = format_column(data.sexp, &default_options());
            assert_eq!(formatted, vec![
                ColumnValue::FormattedValue("<numeric [1]>".to_string()),
                FormattedValue::NULL.into(),
                ColumnValue::FormattedValue("<numeric [1]>".to_string())
            ]);
        })
    }

    #[test]
    fn test_integer_formatting() {
        r_test(|| {
            let data = r_parse_eval0(
                "as.integer(c(1, 1000, 0, -100000, NA, 1000000))",
                R_ENVS.global,
            )
            .unwrap();
            let formatted = format_column(data.sexp, &default_options());
            assert_eq!(formatted, vec![
                ColumnValue::FormattedValue("1".to_string()),
                ColumnValue::FormattedValue("1,000".to_string()),
                ColumnValue::FormattedValue("0".to_string()),
                ColumnValue::FormattedValue("-100,000".to_string()),
                FormattedValue::NA.into(),
                ColumnValue::FormattedValue("1,000,000".to_string())
            ]);
        })
    }

    #[test]
    fn test_chr_formatting() {
        r_test(|| {
            let data = r_parse_eval0("c('a', 'b', 'c', NA, 'd', 'e')", R_ENVS.global).unwrap();
            let formatted = format_column(data.sexp, &default_options());
            assert_eq!(formatted, vec![
                ColumnValue::FormattedValue("a".to_string()),
                ColumnValue::FormattedValue("b".to_string()),
                ColumnValue::FormattedValue("c".to_string()),
                FormattedValue::NA.into(),
                ColumnValue::FormattedValue("d".to_string()),
                ColumnValue::FormattedValue("e".to_string())
            ]);
        })
    }

    #[test]
    fn test_factors_formatting() {
        r_test(|| {
            let data =
                r_parse_eval0("factor(c('aaaaa', 'b', 'c', NA, 'd', 'e'))", R_ENVS.global).unwrap();
            let formatted = format_column(data.sexp, &default_options());
            assert_eq!(formatted, vec![
                ColumnValue::FormattedValue("aaaaa".to_string()),
                ColumnValue::FormattedValue("b".to_string()),
                ColumnValue::FormattedValue("c".to_string()),
                FormattedValue::NA.into(),
                ColumnValue::FormattedValue("d".to_string()),
                ColumnValue::FormattedValue("e".to_string())
            ]);
        })
    }

    #[test]
    fn test_cpl_formatting() {
        // TODO: In the future we might want to use scientific notation for complex numbers
        // although I'm not sure it's really helpful:
        // > 1000000000+1000000000i
        // [1] 1e+09+1e+09i
        r_test(|| {
            let data = r_parse_eval0(
                "c(1+1i, 2+2i, 3+3i, NA, 1000000000+1000000000i, 5+5i)",
                R_ENVS.global,
            )
            .unwrap();
            let formatted = format_column(data.sexp, &default_options());
            assert_eq!(formatted, vec![
                ColumnValue::FormattedValue("1+1i".to_string()),
                ColumnValue::FormattedValue("2+2i".to_string()),
                ColumnValue::FormattedValue("3+3i".to_string()),
                FormattedValue::NA.into(),
                ColumnValue::FormattedValue("1000000000+1000000000i".to_string()),
                ColumnValue::FormattedValue("5+5i".to_string())
            ]);
        })
    }

    #[test]
    fn test_lgl_formatting() {
        r_test(|| {
            let data =
                r_parse_eval0("c(TRUE, FALSE, NA, TRUE, FALSE, TRUE)", R_ENVS.global).unwrap();
            let formatted = format_column(data.sexp, &default_options());
            assert_eq!(formatted, vec![
                ColumnValue::FormattedValue("TRUE".to_string()),
                ColumnValue::FormattedValue("FALSE".to_string()),
                FormattedValue::NA.into(),
                ColumnValue::FormattedValue("TRUE".to_string()),
                ColumnValue::FormattedValue("FALSE".to_string()),
                ColumnValue::FormattedValue("TRUE".to_string())
            ]);
        })
    }

    #[test]
    fn test_date_formatting() {
        r_test(|| {
            let data = r_parse_eval0(
                r#"as.POSIXct(c("2012-01-01", NA, "2017-05-27"))"#,
                R_ENVS.global,
            )
            .unwrap();
            let formatted = format_column(data.sexp, &default_options());
            assert_eq!(formatted, vec![
                ColumnValue::FormattedValue("2012-01-01".to_string()),
                FormattedValue::NA.into(),
                ColumnValue::FormattedValue("2017-05-27".to_string())
            ]);

            let data = r_parse_eval0(
                r#"as.POSIXct(c("2012-01-01 00:01:00", NA, "2017-05-27 00:00:01"))"#,
                R_ENVS.global,
            )
            .unwrap();
            let formatted = format_column(data.sexp, &default_options());
            assert_eq!(formatted, vec![
                ColumnValue::FormattedValue("2012-01-01 00:01:00".to_string()),
                FormattedValue::NA.into(),
                ColumnValue::FormattedValue("2017-05-27 00:00:01".to_string())
            ]);
        })
    }

    #[test]
    fn test_truncation() {
        r_test(|| {
            let mut options = default_options();
            options.max_value_length = 3;

            let data = r_parse_eval0(r#"c("aaaaa", "aaaaaaaa", "aa")"#, R_ENVS.global).unwrap();
            let formatted = format_column(data.sexp, &options);
            assert_eq!(formatted, vec![
                ColumnValue::FormattedValue("aaa".to_string()),
                ColumnValue::FormattedValue("aaa".to_string()),
                ColumnValue::FormattedValue("aa".to_string()),
            ]);

            let data = r_parse_eval0(r#"c("ボルテックス")"#, R_ENVS.global).unwrap();
            let formatted = format_column(data.sexp, &options);
            assert_eq!(formatted, vec![ColumnValue::FormattedValue(
                "ボルテ".to_string()
            ),]);

            options.max_value_length = 4;
            let data = r_parse_eval0(r#"c("नमस्ते")"#, R_ENVS.global).unwrap();
            let formatted = format_column(data.sexp, &options);
            assert_eq!(formatted, vec![ColumnValue::FormattedValue(
                "नमस्".to_string()
            ),]);
        })
    }
}
