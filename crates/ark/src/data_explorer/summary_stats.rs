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

use crate::data_explorer::format::format_string;
use crate::modules::ARK_ENVS;

pub fn summary_stats(
    column: SEXP,
    display_type: ColumnDisplayType,
    format_options: &FormatOptions,
) -> ColumnSummaryStats {
    match summary_stats_(column, display_type, format_options) {
        Ok(stats) => stats,
        Err(e) => {
            // We want to log the error but return an empty summary stats so
            // that the user can still see the rest of the data.
            log::error!("Error getting summary stats: {e:?}");
            empty_column_summary_stats()
        },
    }
}

fn summary_stats_(
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
        .zip(values.into_iter())
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
        num_empty: get_stat(&r_stats, "num_empty")? as i64,
        num_unique: get_stat(&r_stats, "num_unique")? as i64,
    })
}

fn summary_stats_boolean(column: SEXP) -> anyhow::Result<SummaryStatsBoolean> {
    let stats = call_summary_fn("summary_stats_boolean", column)?;
    let r_stats: HashMap<String, i32> = stats.try_into()?;

    Ok(SummaryStatsBoolean {
        true_count: get_stat(&r_stats, "true_count")? as i64,
        false_count: get_stat(&r_stats, "false_count")? as i64,
    })
}

fn summary_stats_date(column: SEXP) -> anyhow::Result<SummaryStatsDate> {
    let r_stats: HashMap<String, RObject> =
        call_summary_fn("summary_stats_date", column)?.try_into()?;

    let num_unique: i32 = get_stat(&r_stats, "num_unique")?.try_into()?;

    Ok(SummaryStatsDate {
        min_date: get_stat(&r_stats, "min_date")?.try_into()?,
        mean_date: get_stat(&r_stats, "mean_date")?.try_into()?,
        median_date: get_stat(&r_stats, "median_date")?.try_into()?,
        max_date: get_stat(&r_stats, "max_date")?.try_into()?,
        num_unique: num_unique as i64,
    })
}

fn summary_stats_datetime(column: SEXP) -> anyhow::Result<SummaryStatsDatetime> {
    // Use the same implementationas the date summary stats
    // but add the timezone.
    let r_stats: HashMap<String, RObject> =
        call_summary_fn("summary_stats_date", column)?.try_into()?;

    let num_unique: i32 = get_stat(&r_stats, "num_unique")?.try_into()?;
    let timezone: Option<String> = RFunction::from("summary_stats_get_timezone")
        .add(column)
        .call_in(ARK_ENVS.positron_ns)?
        .try_into()?;

    Ok(SummaryStatsDatetime {
        min_date: get_stat(&r_stats, "min_date")?.try_into()?,
        mean_date: get_stat(&r_stats, "mean_date")?.try_into()?,
        median_date: get_stat(&r_stats, "median_date")?.try_into()?,
        max_date: get_stat(&r_stats, "max_date")?.try_into()?,
        num_unique: num_unique as i64,
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

fn get_stat<T: Clone>(stats: &HashMap<String, T>, name: &str) -> anyhow::Result<T> {
    let value = stats.get(name);

    match value {
        Some(value) => Ok(value.clone()),
        None => Err(anyhow!("Missing stat {}", name)),
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
        }
    }

    #[test]
    fn test_numeric_summary() {
        r_test(|| {
            let column = r_parse_eval0("c(1,2,3,4,5, NA)", R_ENVS.global).unwrap();
            let stats =
                summary_stats_(column.sexp, ColumnDisplayType::Number, &default_options()).unwrap();
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
    fn test_string_summary() {
        r_test(|| {
            let column = r_parse_eval0("c('a', 'b', 'c', 'd', '')", R_ENVS.global).unwrap();
            let stats =
                summary_stats_(column.sexp, ColumnDisplayType::String, &default_options()).unwrap();
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
            let column = r_parse_eval0("c(TRUE, FALSE, TRUE, TRUE, NA)", R_ENVS.global).unwrap();
            let stats = summary_stats_(column.sexp, ColumnDisplayType::Boolean, &default_options())
                .unwrap();
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
            let column = r_parse_eval0(
                "as.Date(c('2021-01-01', '2021-01-02', '2021-01-03', '2021-01-04', NA))",
                R_ENVS.global,
            )
            .unwrap();
            let stats =
                summary_stats_(column.sexp, ColumnDisplayType::Date, &default_options()).unwrap();
            let expected = SummaryStatsDate {
                min_date: "2021-01-01".to_string(),
                mean_date: "2021-01-02".to_string(),
                median_date: "2021-01-02".to_string(),
                max_date: "2021-01-04".to_string(),
                num_unique: 5,
            };
            assert_eq!(stats.date_stats, Some(expected));
        })
    }

    #[test]
    fn test_datetime_summary() {
        r_test(|| {
            let column = r_parse_eval0(
                "as.POSIXct(c('2015-07-24 23:15:07', '2015-07-24 23:15:07', NA), tz = 'Japan')",
                R_ENVS.global,
            )
            .unwrap();
            let stats =
                summary_stats_(column.sexp, ColumnDisplayType::Datetime, &default_options())
                    .unwrap();
            let expected = SummaryStatsDatetime {
                num_unique: 2,
                min_date: "2015-07-24 23:15:07".to_string(),
                mean_date: "2015-07-24 23:15:07".to_string(),
                median_date: "2015-07-24 23:15:07".to_string(),
                max_date: "2015-07-24 23:15:07".to_string(),
                timezone: Some("Japan".to_string()),
            };
            assert_eq!(stats.datetime_stats, Some(expected));
        })
    }
}
