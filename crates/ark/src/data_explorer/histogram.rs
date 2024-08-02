use std::collections::HashMap;

use amalthea::comm::data_explorer_comm::ColumnHistogram;
use amalthea::comm::data_explorer_comm::ColumnHistogramParams;
use amalthea::comm::data_explorer_comm::ColumnQuantileValue;
use amalthea::comm::data_explorer_comm::FormatOptions;
use anyhow::anyhow;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_null;
use harp::utils::r_classes;
use harp::utils::r_inherits;
use harp::utils::r_typeof;
use libr::INTSXP;
use libr::REALSXP;
use libr::SEXP;
use stdext::*;

use crate::data_explorer::format::format_string;
use crate::modules::ARK_ENVS;

pub fn profile_histogram(
    column: SEXP,
    params: &ColumnHistogramParams,
    format_options: &FormatOptions,
) -> anyhow::Result<ColumnHistogram> {
    let quantiles: RObject = match params.quantiles.clone() {
        Some(v) => (&v).into(),
        None => r_null().into(),
    };

    // Checks for supported objects:
    // - Atomic integers and doubles
    // - Dates and POSIXct objects
    match r_classes(column) {
        Some(v) => {
            if !r_inherits(column, "Date") && !r_inherits(column, "POSIXct") {
                return Err(anyhow!("Object with class '{:?}' unsupported.", v));
            }
        },
        None => match r_typeof(column) {
            INTSXP | REALSXP => {},
            _ => return Err(anyhow!("Type not supported {:?}", r_typeof(column))),
        },
    }

    let results: HashMap<String, RObject> = RFunction::from("profile_histogram")
        .add(column)
        .add(params.num_bins as i32)
        .add(quantiles)
        .call_in(ARK_ENVS.positron_ns)?
        .try_into()?;

    // Bin edges are expected to be objects that can be formatted, such as integers vectors,
    // numeric vectors or even dates.
    let bin_edges = unwrap!(results.get("bin_edges"), None => {
        return Err(anyhow!("`bin_edges` were not computed."));
    });
    let bin_edges_formatted = format_string(bin_edges.sexp, &format_options);

    // The quantile values should also be formattable
    let quantile_values = unwrap!(results.get("quantiles"), None => {
        return Err(anyhow!("`quantiles` were not computed"));
    });
    let quantile_values_formatted = format_string(quantile_values.sexp, &format_options);

    // Counts the amount of elements for each bin.
    let bin_counts: Vec<i32> = unwrap!(results.get("bin_counts"), None => {
        return Err(anyhow!("`bin_counts` were not computed."))
    })
    .clone()
    .try_into()?;

    if bin_counts.len() != (bin_edges_formatted.len() - 1) {
        return Err(anyhow!(
            "`bin_counts` not compatible with `bin_edges`. `bin_counts.len()` ({}) and `bin_edges_formatted.len()` ({})",
            bin_counts.len(),
            bin_edges_formatted.len()
        ));
    }

    // Computed quantile values are combined with the request probs to form
    // ColumnQuantileValue's.
    let quantiles = params
        .quantiles
        .clone()
        .unwrap_or(vec![])
        .into_iter()
        .zip(quantile_values_formatted.into_iter())
        .map(|(q, value)| ColumnQuantileValue {
            q,
            value,
            exact: true,
        })
        .collect();

    Ok(ColumnHistogram {
        bin_edges: bin_edges_formatted,
        bin_counts: bin_counts.into_iter().map(|v| v as i64).collect(),
        quantiles,
    })
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

    fn test_histogram(code: &str, num_bins: i64, bin_edges: Vec<&str>, bin_counts: Vec<i64>) {
        let column = r_parse_eval0(code, R_ENVS.global).unwrap();

        let hist = profile_histogram(
            column.sexp,
            &ColumnHistogramParams {
                num_bins,
                quantiles: None,
            },
            &default_options(),
        )
        .unwrap();

        assert_eq!(hist, ColumnHistogram {
            bin_edges: bin_edges.into_iter().map(|v| v.to_string()).collect(),
            bin_counts,
            quantiles: vec![]
        })
    }

    #[test]
    fn test_basic_histograms() {
        r_test(|| {
            test_histogram(
                "0:10",
                4,
                vec!["0.00", "2.50", "5.00", "7.50", "10.00"],
                vec![3, 3, 2, 3],
            );
        })
    }

    #[test]
    fn test_date_histogram() {
        r_test(|| {
            test_histogram(
                "seq(as.Date('2000-01-01'), by = 1, length.out = 11)",
                4,
                vec![
                    "2000-01-01 00:00:00",
                    "2000-01-03 12:00:00",
                    "2000-01-06 00:00:00",
                    "2000-01-08 12:00:00",
                    "2000-01-11 00:00:00",
                ],
                vec![3, 2, 3, 3],
            );

            test_histogram(
                "rep(seq(as.Date('2000-01-01'), by = 1, length.out = 2), 100)",
                10,
                vec![
                    "2000-01-01 00:00:00",
                    "2000-01-01 12:00:00",
                    "2000-01-02 00:00:00",
                ],
                vec![100, 100],
            );

            test_histogram(
                "rep(seq(as.Date('2000-01-01'), by = 2, length.out = 2), 100)",
                10,
                vec![
                    "2000-01-01 00:00:00",
                    "2000-01-01 16:00:00",
                    "2000-01-02 08:00:00",
                    "2000-01-03 00:00:00",
                ],
                vec![100, 0, 100],
            );
        })
    }

    #[test]
    fn test_constant_column() {
        r_test(|| {
            test_histogram("c(1, 1, 1)", 4, vec!["1.00", "1.00"], vec![3]);
        })
    }

    #[test]
    fn test_integers() {
        r_test(|| {
            test_histogram(
                "rep(c(1L, 2L), 100)",
                5,
                vec!["1.00", "1.20", "1.40", "1.60", "1.80", "2.00"],
                vec![100, 0, 0, 0, 100],
            );

            test_histogram(
                "rep(c(1L, 3L), 100)",
                3,
                vec!["1.00", "1.67", "2.33", "3.00"],
                vec![100, 0, 100],
            );

            test_histogram(
                "rep(c(1L, 3L), 100)",
                2,
                vec!["1.00", "2.00", "3.00"],
                vec![100, 100],
            );
        })
    }

    #[test]
    fn test_posixct() {
        r_test(|| {
            test_histogram(
                // 1 sec, is the difference of 1 in the numeric data representation
                // R doesn't distinguish changes in the decimal places as different dates
                "rep(seq(as.POSIXct('2017-05-17 00:00:00'), by = '1 sec', length.out = 4), 10)",
                10,
                vec![
                    "2017-05-17 00:00:00",
                    "2017-05-17 00:00:00",
                    "2017-05-17 00:00:01",
                    "2017-05-17 00:00:02",
                    "2017-05-17 00:00:03",
                ],
                vec![10, 10, 10, 10],
            );
        })
    }
}
