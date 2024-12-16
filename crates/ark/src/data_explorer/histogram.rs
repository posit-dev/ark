use std::collections::HashMap;

use amalthea::comm::data_explorer_comm::ColumnFrequencyTable;
use amalthea::comm::data_explorer_comm::ColumnFrequencyTableParams;
use amalthea::comm::data_explorer_comm::ColumnHistogram;
use amalthea::comm::data_explorer_comm::ColumnHistogramParams;
use amalthea::comm::data_explorer_comm::ColumnHistogramParamsMethod;
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

    let num_bins: RObject = (params.num_bins as i32).into();

    let method: RObject = match params.method {
        ColumnHistogramParamsMethod::Fixed => "fixed".into(),
        ColumnHistogramParamsMethod::Sturges => "sturges".into(),
        ColumnHistogramParamsMethod::FreedmanDiaconis => "fd".into(),
        ColumnHistogramParamsMethod::Scott => "scott".into(),
    };

    let results: HashMap<String, RObject> = RFunction::from("profile_histogram")
        .add(column)
        .add(method)
        .add(num_bins)
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

    if bin_counts.len() > 0 && bin_counts.len() != (bin_edges_formatted.len() - 1) {
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

pub fn profile_frequency_table(
    column: SEXP,
    params: &ColumnFrequencyTableParams,
    format_options: &FormatOptions,
) -> anyhow::Result<ColumnFrequencyTable> {
    let results: HashMap<String, RObject> = RFunction::from("profile_frequency_table")
        .add(column)
        .add(params.limit as i32)
        .call_in(ARK_ENVS.positron_ns)?
        .try_into()?;

    let values = unwrap!(results.get("values"), None => {
        return Err(anyhow!("Something went wrong when computing `values`"));
    });
    let values_formatted = format_string(values.sexp, format_options);

    let counts: Vec<i32> = unwrap!(results.get("counts"), None => {
        return Err(anyhow!("Something went wrong when computing `counts`"));
    })
    .clone()
    .try_into()?;

    let other_count = if counts.len() == params.limit as usize {
        let val: i32 = unwrap!(results.get("other_count"), None => {
            return Err(anyhow!("Something went wrong when computing `others_count`"))
        })
        .clone()
        .try_into()?;
        Some(val as i64)
    } else {
        None
    };

    Ok(ColumnFrequencyTable {
        values: values_formatted,
        counts: counts.into_iter().map(|v| v as i64).collect(),
        other_count,
    })
}

#[cfg(test)]
mod tests {
    use harp::object::RObject;
    use stdext::assert_match;

    use super::*;
    use crate::fixtures::package_is_installed;
    use crate::r_task;

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
        let column = harp::parse_eval_global(code).unwrap();

        let hist = profile_histogram(
            column.sexp,
            &ColumnHistogramParams {
                method: ColumnHistogramParamsMethod::Fixed,
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

    fn test_histogram_method(code: &str, method: &str, bin_edges: Vec<&str>, bin_counts: Vec<i64>) {
        let method = if method == "sturges" {
            ColumnHistogramParamsMethod::Sturges
        } else if method == "fd" {
            ColumnHistogramParamsMethod::FreedmanDiaconis
        } else if method == "scott" {
            ColumnHistogramParamsMethod::Scott
        } else {
            panic!("No method with this name");
        };

        let column = harp::parse_eval_global(code).unwrap();

        let hist = profile_histogram(
            column.sexp,
            &ColumnHistogramParams {
                method,
                num_bins: 100000,
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

    fn test_quantiles<T>(code: &str, quantiles: Vec<f64>, expected: T)
    where
        RObject: From<T>,
    {
        let column = harp::parse_eval_global(code).unwrap();

        let hist = profile_histogram(
            column.sexp,
            &ColumnHistogramParams {
                method: ColumnHistogramParamsMethod::Fixed,
                num_bins: 100,
                quantiles: Some(quantiles),
            },
            &default_options(),
        )
        .unwrap();

        assert_match!(hist, ColumnHistogram { quantiles, .. }  => {
            format_string(RObject::try_from(expected).unwrap().sexp, &default_options()).
            into_iter().
            zip(quantiles.into_iter()).
            for_each(|(expected, quantile)| {
                assert_eq!(expected, quantile.value);
            });
        });
    }

    fn test_frequency_table<T>(
        code: &str,
        limit: i64,
        values: T,
        counts: Vec<i64>,
        other_count: Option<i64>,
    ) where
        RObject: From<T>,
    {
        let column = harp::parse_eval_global(code).unwrap();
        let freq_table = profile_frequency_table(
            column.sexp,
            &ColumnFrequencyTableParams { limit },
            &default_options(),
        )
        .unwrap();

        assert_eq!(freq_table, ColumnFrequencyTable {
            values: format_string(RObject::try_from(values).unwrap().sexp, &default_options()),
            counts,
            other_count
        });
    }

    #[test]
    fn test_basic_histograms() {
        r_task(|| {
            test_histogram("0:10", 5, vec!["0", "2", "4", "6", "8", "10"], vec![
                3, 2, 2, 2, 2,
            ]);
            test_histogram_method(
                "0:10",
                "sturges",
                vec!["0", "2", "4", "6", "8", "10"],
                vec![3, 2, 2, 2, 2],
            );
            test_histogram_method("0:10", "scott", vec!["0", "5", "10"], vec![6, 5]);
            test_histogram_method("0:10", "fd", vec!["0.00", "3.33", "6.67", "10.00"], vec![
                4, 3, 4,
            ]);
        })
    }

    #[test]
    fn test_date_histogram() {
        r_task(|| {
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
                vec![3, 3, 2, 3],
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

            test_histogram_method(
                "rep(seq(as.Date('2000-01-01'), by = 2, length.out = 2), 100)",
                "sturges",
                vec![
                    "2000-01-01 00:00:00",
                    "2000-01-01 16:00:00",
                    "2000-01-02 08:00:00",
                    "2000-01-03 00:00:00",
                ],
                vec![100, 0, 100],
            );

            test_histogram_method(
                "rep(seq(as.Date('2000-01-01'), by = 2, length.out = 2), 100)",
                "fd",
                vec![
                    "2000-01-01 00:00:00",
                    "2000-01-01 16:00:00",
                    "2000-01-02 08:00:00",
                    "2000-01-03 00:00:00",
                ],
                vec![100, 0, 100],
            );

            test_histogram_method(
                "rep(seq(as.Date('2000-01-01'), by = 2, length.out = 2), 100)",
                "scott",
                vec!["2000-01-01", "2000-01-03"],
                vec![200],
            );
        })
    }

    #[test]
    fn test_constant_column() {
        r_task(|| {
            // This is the default `hist` behavior, single bin containing all info.
            test_histogram("c(1, 1, 1)", 4, vec!["0.00", "1.00"], vec![3]);
            test_histogram_method("c(1, 1, 1)", "sturges", vec!["0.00", "1.00"], vec![3])
        })
    }

    #[test]
    fn test_integers() {
        r_task(|| {
            test_histogram(
                "rep(c(1L, 2L), 100)",
                5,
                vec!["1.00", "1.50", "2.00"],
                vec![100, 100],
            );

            test_histogram(
                "rep(c(1L, 3L), 100)",
                3,
                vec!["1.00", "1.67", "2.33", "3.00"],
                vec![100, 0, 100],
            );

            test_histogram("rep(c(1L, 3L), 100)", 2, vec!["1", "2", "3"], vec![
                100, 100,
            ]);

            test_histogram_method(
                "rep(c(1L, 3L), 100)",
                "sturges",
                vec!["1.00", "1.67", "2.33", "3.00"],
                vec![100, 0, 100],
            );
        })
    }

    #[test]
    fn test_posixct() {
        r_task(|| {
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

            test_histogram_method(
                "rep(seq(as.POSIXct('2017-05-17 00:00:00'), by = '1 sec', length.out = 4), 10)",
                "sturges",
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

    #[test]
    fn test_quantile_numerics() {
        r_task(|| {
            test_quantiles("c(1,2,3,4,5)", vec![0.5], &vec![3.0]);
            test_quantiles("c(1L,2L,3L,4L,5L)", vec![0.5], &vec![3.0]);
            test_quantiles("c(0.1,0.1,0.1,0.1,0.1)", vec![0.5, 0.9], &vec![0.1, 0.1]);
            test_quantiles("c(1, 2)", vec![0., 0.5, 1.], &vec![1., 1.5, 2.]);

            // Get NA's when data is just NA's
            test_quantiles(
                "c(NA_real_, NA_real_)",
                vec![0.5, 0.9],
                harp::parse_eval_global("c(NA_real_, NA_real_)").unwrap(),
            );

            // Get constant value when there's a single non-na value
            test_quantiles(
                "c(1, NA_real_)",
                vec![0.5, 0.9],
                harp::parse_eval_global("c(1, 1)").unwrap(),
            );

            // Make sure Inf, -Inf and NaN are also ignored
            test_quantiles(
                "c(1, NaN, Inf, -Inf)",
                vec![0.5, 0.9],
                harp::parse_eval_global("c(1, 1)").unwrap(),
            );
        });
    }

    #[test]
    fn test_quantiles_dates() {
        r_task(|| {
            test_quantiles(
                "as.Date(c('2010-01-01', '2010-01-02', '2010-01-03'))",
                vec![0.5],
                harp::parse_eval_global("as.Date('2010-01-02')").unwrap(),
            );
            test_quantiles(
                "as.Date(c('2010-01-01', '2010-01-02'))",
                vec![0.5],
                harp::parse_eval_global("as.POSIXct('2010-01-01 12:00:00')").unwrap(),
            );

            // What happens when there's no representable dates between min and max.
            test_quantiles(
                "as.POSIXct(c('2010-01-01 00:00:01', '2010-01-01 00:00:02'))",
                vec![0.5],
                harp::parse_eval_global("as.POSIXct('2010-01-01 00:00:01')").unwrap(),
            );

            // NA's are ignored
            test_quantiles(
                "as.Date(c('2010-01-01', '2010-01-02', NA))",
                vec![0.5],
                harp::parse_eval_global("as.POSIXct('2010-01-01 12:00:00')").unwrap(),
            );
        })
    }

    #[test]
    fn test_frequency_table_strings() {
        r_task(|| {
            test_frequency_table(
                "c(rep('a', 100), rep('b', 200), rep('c', 150))",
                10,
                harp::parse_eval_global("c('b', 'c', 'a')").unwrap(),
                vec![200, 150, 100],
                None,
            );
            test_frequency_table(
                "c(rep('a', 100), rep('b', 200), rep('c', 150))",
                2,
                harp::parse_eval_global("c('b', 'c')").unwrap(),
                vec![200, 150],
                Some(100),
            );

            // NA's do not count
            test_frequency_table(
                "c(rep('a', 100), rep('b', 200), rep('c', 150), NA)",
                10,
                harp::parse_eval_global("c('b', 'c', 'a')").unwrap(),
                vec![200, 150, 100],
                None,
            );
        })
    }

    #[test]
    fn test_frequency_table_factors() {
        r_task(|| {
            test_frequency_table(
                "factor(c(rep('a', 100), rep('b', 200), rep('c', 150)))",
                10,
                harp::parse_eval_global("c('b', 'c', 'a')").unwrap(),
                vec![200, 150, 100],
                None,
            );
            test_frequency_table(
                "factor(c(rep('a', 100), rep('b', 200), rep('c', 150)))",
                2,
                harp::parse_eval_global("c('b', 'c')").unwrap(),
                vec![200, 150],
                Some(100),
            );

            // Account for all factor levels, even if they don't appear in the data
            test_frequency_table(
                "factor(rep(c('a', 'b'), c(100, 200)), levels = c('a', 'b', 'c'))",
                10,
                harp::parse_eval_global("c('b', 'a', 'c')").unwrap(),
                vec![200, 100, 0],
                None,
            );
        })
    }

    #[test]
    fn test_frequency_table_numerics_and_dates() {
        r_task(|| {
            test_frequency_table(
                "rep(0:10/10, 1:11)",
                100,
                harp::parse_eval_global("10:0/10").unwrap(),
                vec![11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1],
                None,
            );

            test_frequency_table(
                "rep(0:10/10, 1:11)",
                5,
                harp::parse_eval_global("10:6/10").unwrap(),
                vec![11, 10, 9, 8, 7],
                Some(21),
            );

            // Inf and -Inf appear as levels but not NA's or NaN
            test_frequency_table(
                "c(NaN, Inf, -Inf, NA)",
                5,
                harp::parse_eval_global("c(Inf, -Inf)").unwrap(),
                vec![1, 1],
                None,
            );

            // Works with integers
            test_frequency_table(
                "rep(0:10, 1:11)",
                5,
                harp::parse_eval_global("10:6").unwrap(),
                vec![11, 10, 9, 8, 7],
                Some(21),
            );

            // Works with dates
            test_frequency_table(
                "as.POSIXct(rep(c('2010-01-01', '2017-05-17 11:00:00'), c(100, 200)))",
                5,
                harp::parse_eval_global("as.POSIXct(c('2017-05-17 11:00:00','2010-01-01'))")
                    .unwrap(),
                vec![200, 100],
                None,
            );
        })
    }

    #[test]
    fn test_frequency_table_haven_labelled() {
        r_task(|| {
            if !package_is_installed("haven") {
                return;
            }

            test_frequency_table(
                "haven::labelled(c(rep(1, 100), rep(2, 200), rep(3, 150)), labels = c('A' = 1, 'B' = 2, 'C' = 3))",
                10,
                harp::parse_eval_global("c('B', 'C', 'A')").unwrap(),
                vec![200, 150, 100],
                None,
            );
            // Account for all factor levels, even if they don't appear in the data
            test_frequency_table(
                "haven::labelled(c(rep(1, 100), rep(2, 200)), labels = c('A' = 1, 'B' = 2, 'C' = 3))",
                10,
                harp::parse_eval_global("c('B', 'A', 'C')").unwrap(),
                vec![200, 100, 0],
                None,
            );
        })
    }

    #[test]
    fn test_limit_bins() {
        // Regression test for https://github.com/posit-dev/positron/issues/5744
        r_task(|| {
            let column = harp::parse_eval_global("rep(c(1:10, 5e7), 10)").unwrap();

            let hist = profile_histogram(
                column.sexp,
                &ColumnHistogramParams {
                    method: ColumnHistogramParamsMethod::FreedmanDiaconis,
                    // If num_bins wasn't set to 10, that would generate almost 200k bins
                    num_bins: 10,
                    quantiles: None,
                },
                &default_options(),
            )
            .unwrap();

            assert_match!(hist, ColumnHistogram { bin_edges, bin_counts, .. } => {
                assert_eq!(bin_edges.len(), 11);
                assert_eq!(bin_counts.len(), 10);
            });
        })
    }
}
