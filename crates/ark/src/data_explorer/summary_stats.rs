//
// summary_stats.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use std::collections::HashMap;

use amalthea::comm::data_explorer_comm;
use amalthea::comm::data_explorer_comm::ColumnDisplayType;
use amalthea::comm::data_explorer_comm::ColumnSummaryStats;
use amalthea::comm::data_explorer_comm::FormatOptions;
use amalthea::comm::data_explorer_comm::SummaryStatsBoolean;
use amalthea::comm::data_explorer_comm::SummaryStatsDate;
use amalthea::comm::data_explorer_comm::SummaryStatsDatetime;
use amalthea::comm::data_explorer_comm::SummaryStatsNumber;
use amalthea::comm::data_explorer_comm::SummaryStatsString;
use anyhow::anyhow;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::utils::r_names2;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use libr::SEXP;
use stdext::unwrap;

use crate::data_explorer::format::format_string;
use crate::modules::ARK_ENVS;

pub fn summary_stats(
    column: SEXP,
    display_type: ColumnDisplayType,
    format_options: &FormatOptions,
) -> anyhow::Result<ColumnSummaryStats> {
    let mut stats = empty_column_summary_stats();
    stats.type_display = display_type;
    match stats.type_display {
        ColumnDisplayType::Number => {
            stats.number_stats = Some(summary_stats_number(column, format_options)?);
        },
        ColumnDisplayType::String => {
            stats.string_stats = Some(summary_stats_string(column)?);
        },
        ColumnDisplayType::Boolean => {
            stats.boolean_stats = Some(summary_stats_boolean(column)?);
        },
        ColumnDisplayType::Date => stats.date_stats = Some(summary_stats_date(column)?),
        ColumnDisplayType::Datetime => stats.datetime_stats = Some(summary_stats_datetime(column)?),
        _ => {
            return Err(anyhow::anyhow!("Unkown type"));
        },
    };
    Ok(stats)
}

fn summary_stats_number(
    column: SEXP,
    format_options: &FormatOptions,
) -> anyhow::Result<SummaryStatsNumber> {
    let r_stats = call_summary_fn("summary_stats_number", column)?;

    let names = unsafe { CharacterVector::new_unchecked(r_names2(r_stats.sexp)) };
    let values = format_string(r_stats.sexp, format_options);

    let r_stats: HashMap<String, String> = names
        .iter()
        .zip(values)
        .map(|(name, value)| match name {
            Some(name) => (name, value),
            None => ("unk".to_string(), value),
        })
        .collect();

    Ok(SummaryStatsNumber {
        min_value: r_stats.get("min_value").cloned(),
        max_value: r_stats.get("max_value").cloned(),
        mean: r_stats.get("mean").cloned(),
        median: r_stats.get("median").cloned(),
        stdev: r_stats.get("stdev").cloned(),
    })
}

fn summary_stats_string(column: SEXP) -> anyhow::Result<SummaryStatsString> {
    let stats = call_summary_fn("summary_stats_string", column)?;
    let r_stats: HashMap<String, i32> = stats.try_into()?;

    Ok(SummaryStatsString {
        num_empty: get_stat(&r_stats, "num_empty")?,
        num_unique: get_stat(&r_stats, "num_unique")?,
    })
}

fn summary_stats_boolean(column: SEXP) -> anyhow::Result<SummaryStatsBoolean> {
    let stats = call_summary_fn("summary_stats_boolean", column)?;
    let r_stats: HashMap<String, i32> = stats.try_into()?;

    Ok(SummaryStatsBoolean {
        true_count: get_stat(&r_stats, "true_count")?,
        false_count: get_stat(&r_stats, "false_count")?,
    })
}

fn summary_stats_date(column: SEXP) -> anyhow::Result<SummaryStatsDate> {
    let r_stats: HashMap<String, RObject> =
        call_summary_fn("summary_stats_date", column)?.try_into()?;

    let num_unique: Option<i64> = get_stat::<i32, RObject>(&r_stats, "num_unique")
        .ok()
        .and_then(|x| Some(x as i64));

    Ok(SummaryStatsDate {
        min_date: get_stat(&r_stats, "min_date").ok(),
        mean_date: get_stat(&r_stats, "mean_date").ok(),
        median_date: get_stat(&r_stats, "median_date").ok(),
        max_date: get_stat(&r_stats, "max_date").ok(),
        num_unique,
    })
}

fn summary_stats_datetime(column: SEXP) -> anyhow::Result<SummaryStatsDatetime> {
    // Use the same implementationas the date summary stats
    // but add the timezone.
    let r_stats: HashMap<String, RObject> =
        call_summary_fn("summary_stats_date", column)?.try_into()?;

    let num_unique: Option<i64> = get_stat::<i32, RObject>(&r_stats, "num_unique")
        .ok()
        .and_then(|x| Some(x as i64));

    let timezone: Option<String> = RFunction::from("summary_stats_get_timezone")
        .add(column)
        .call_in(ARK_ENVS.positron_ns)?
        .try_into()?;

    Ok(SummaryStatsDatetime {
        min_date: get_stat(&r_stats, "min_date").ok(),
        mean_date: get_stat(&r_stats, "mean_date").ok(),
        median_date: get_stat(&r_stats, "median_date").ok(),
        max_date: get_stat(&r_stats, "max_date").ok(),
        num_unique,
        timezone,
    })
}

fn call_summary_fn(function: &str, column: SEXP) -> anyhow::Result<RObject> {
    Ok(RFunction::from(function)
        .add(column)
        .call_in(ARK_ENVS.positron_ns)?)
}

fn empty_column_summary_stats() -> data_explorer_comm::ColumnSummaryStats {
    data_explorer_comm::ColumnSummaryStats {
        type_display: ColumnDisplayType::Unknown,
        number_stats: None,
        string_stats: None,
        boolean_stats: None,
        date_stats: None,
        datetime_stats: None,
    }
}

fn get_stat<Return, T: Clone>(stats: &HashMap<String, T>, name: &str) -> anyhow::Result<Return>
where
    Return: TryFrom<T>,
{
    let value = stats.get(name);

    match value {
        Some(value) => {
            let value: Return = unwrap!(value.clone().try_into(), Err(_) => {
                return Err(anyhow!("Can't cast to return type."))
            });
            Ok(value)
        },
        None => Err(anyhow!("Missing stat {}", name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::r_test;

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
    fn test_numeric_summary() {
        r_test(|| {
            let column = harp::parse_eval_global("c(1,2,3,4,5, NA)").unwrap();
            let stats =
                summary_stats(column.sexp, ColumnDisplayType::Number, &default_options()).unwrap();
            let expected = SummaryStatsNumber {
                min_value: Some("1.00".to_string()),
                max_value: Some("5.00".to_string()),
                mean: Some("3.00".to_string()),
                median: Some("3.00".to_string()),
                stdev: Some("1.58".to_string()),
            };
            assert_eq!(stats.number_stats, Some(expected));
        })
    }

    #[test]
    fn test_numeric_all_nas() {
        r_test(|| {
            let column = harp::parse_eval_global("c(NA_real_, NA_real_, NA_real_)").unwrap();
            let stats =
                summary_stats(column.sexp, ColumnDisplayType::Number, &default_options()).unwrap();
            let expected = SummaryStatsNumber {
                min_value: None,
                max_value: None,
                mean: None,
                median: None,
                stdev: None,
            };
            assert_eq!(stats.number_stats, Some(expected));
        })
    }

    #[test]
    fn test_string_summary() {
        r_test(|| {
            let column = harp::parse_eval_global("c('a', 'b', 'c', 'd', '')").unwrap();
            let stats =
                summary_stats(column.sexp, ColumnDisplayType::String, &default_options()).unwrap();
            let expected = SummaryStatsString {
                num_empty: 1,
                num_unique: 5,
            };
            assert_eq!(stats.string_stats, Some(expected));
        })
    }

    #[test]
    fn test_string_summary_for_factors() {
        r_test(|| {
            let column = harp::parse_eval_global("factor(c('a', 'b', 'c', 'd', ''))").unwrap();
            let stats =
                summary_stats(column.sexp, ColumnDisplayType::String, &default_options()).unwrap();
            let expected = SummaryStatsString {
                num_empty: 1,
                num_unique: 5,
            };
            assert_eq!(stats.string_stats, Some(expected));
        })
    }

    #[test]
    fn test_boolean_summary() {
        r_test(|| {
            let column = harp::parse_eval_global("c(TRUE, FALSE, TRUE, TRUE, NA)").unwrap();
            let stats =
                summary_stats(column.sexp, ColumnDisplayType::Boolean, &default_options()).unwrap();
            let expected = SummaryStatsBoolean {
                true_count: 3,
                false_count: 1,
            };
            assert_eq!(stats.boolean_stats, Some(expected));
        })
    }

    #[test]
    fn test_date_summary() {
        r_test(|| {
            let column = harp::parse_eval_global(
                "as.Date(c('2021-01-01', '2021-01-02', '2021-01-03', '2021-01-04', NA))",
            )
            .unwrap();
            let stats =
                summary_stats(column.sexp, ColumnDisplayType::Date, &default_options()).unwrap();
            let expected = SummaryStatsDate {
                min_date: Some("2021-01-01".to_string()),
                mean_date: Some("2021-01-02".to_string()),
                median_date: Some("2021-01-02".to_string()),
                max_date: Some("2021-01-04".to_string()),
                num_unique: Some(5),
            };
            assert_eq!(stats.date_stats, Some(expected));
        })
    }

    #[test]
    fn test_datetime_summary() {
        r_test(|| {
            let column = harp::parse_eval_global(
                "as.POSIXct(c('2015-07-24 23:15:07', '2015-07-24 23:15:07', NA), tz = 'Japan')",
            )
            .unwrap();
            let stats = summary_stats(column.sexp, ColumnDisplayType::Datetime, &default_options())
                .unwrap();
            let expected = SummaryStatsDatetime {
                num_unique: Some(2),
                min_date: Some("2015-07-24 23:15:07".to_string()),
                mean_date: Some("2015-07-24 23:15:07".to_string()),
                median_date: Some("2015-07-24 23:15:07".to_string()),
                max_date: Some("2015-07-24 23:15:07".to_string()),
                timezone: Some("Japan".to_string()),
            };
            assert_eq!(stats.datetime_stats, Some(expected));
        })
    }

    #[test]
    fn test_date_all_na() {
        r_test(|| {
            let column = harp::parse_eval_base("as.Date(NA)").unwrap();
            let stats =
                summary_stats(column.sexp, ColumnDisplayType::Date, &default_options()).unwrap();
            let expected = SummaryStatsDate {
                num_unique: Some(1),
                min_date: None,
                mean_date: None,
                median_date: None,
                max_date: None,
            };
            assert_eq!(stats.date_stats, Some(expected));
        })
    }
}
