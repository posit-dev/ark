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
) -> ColumnSummaryStats {
    match summary_stats_(column, display_type, format_options) {
        Ok(stats) => stats,
        Err(e) => {
            // We want to log the error but return an empty summary stats so
            // that the user can still see the rest of the data.
            log::error!("Error getting summary stats: {:?}", e);
            empty_column_summary_stats()
        },
    }
}

fn summary_stats_(
    column: SEXP,
    display_type: ColumnDisplayType,
    format_options: &FormatOptions,
) -> anyhow::Result<ColumnSummaryStats> {
    match display_type {
        ColumnDisplayType::Number => Ok(summary_stats_number(column, format_options)?.into()),
        ColumnDisplayType::String => Ok(summary_stats_string(column)?.into()),
        ColumnDisplayType::Boolean => Ok(summary_stats_boolean(column)?.into()),
        ColumnDisplayType::Date => Ok(summary_stats_date(column)?.into()),
        ColumnDisplayType::Datetime => Ok(summary_stats_datetime(column)?.into()),
        _ => Err(anyhow::anyhow!("Unkown type")),
    }
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

    Ok(SummaryStatsNumber(data_explorer_comm::SummaryStatsNumber {
        min_value: get_stat(&r_stats, "min_value")?,
        max_value: get_stat(&r_stats, "max_value")?,
        mean: get_stat(&r_stats, "mean")?,
        median: get_stat(&r_stats, "median")?,
        stdev: get_stat(&r_stats, "stdev")?,
    }))
}

fn summary_stats_string(column: SEXP) -> anyhow::Result<SummaryStatsString> {
    let stats = call_summary_fn("summary_stats_string", column)?;
    let r_stats: HashMap<String, i32> = stats.try_into()?;

    Ok(SummaryStatsString(data_explorer_comm::SummaryStatsString {
        num_empty: get_stat(&r_stats, "num_empty")? as i64,
        num_unique: get_stat(&r_stats, "num_unique")? as i64,
    }))
}

fn summary_stats_boolean(column: SEXP) -> anyhow::Result<SummaryStatsBoolean> {
    let stats = call_summary_fn("summary_stats_boolean", column)?;
    let r_stats: HashMap<String, i32> = stats.try_into()?;

    Ok(SummaryStatsBoolean(
        data_explorer_comm::SummaryStatsBoolean {
            true_count: get_stat(&r_stats, "true_count")? as i64,
            false_count: get_stat(&r_stats, "false_count")? as i64,
        },
    ))
}

fn summary_stats_date(column: SEXP) -> anyhow::Result<SummaryStatsDate> {
    let r_stats: HashMap<String, RObject> =
        call_summary_fn("summary_stats_date", column)?.try_into()?;

    let num_unique: i32 = get_stat(&r_stats, "num_unique")?.try_into()?;

    Ok(SummaryStatsDate(data_explorer_comm::SummaryStatsDate {
        min_date: robj_to_string(&get_stat(&r_stats, "min_date")?),
        mean_date: robj_to_string(&get_stat(&r_stats, "mean_date")?),
        median_date: robj_to_string(&get_stat(&r_stats, "median_date")?),
        max_date: robj_to_string(&get_stat(&r_stats, "max_date")?),
        num_unique: num_unique as i64,
    }))
}

fn summary_stats_datetime(column: SEXP) -> anyhow::Result<SummaryStatsDatetime> {
    // use the same implementationas the date
    let r_stats: HashMap<String, RObject> =
        call_summary_fn("summary_stats_date", column)?.try_into()?;

    let num_unique: i32 = r_stats["num_unique"].clone().try_into()?;
    let timezone: Option<String> = RFunction::from("summary_stats_get_timezone")
        .add(column)
        .call_in(ARK_ENVS.positron_ns)?
        .try_into()?;

    Ok(SummaryStatsDatetime(
        data_explorer_comm::SummaryStatsDatetime {
            min_date: robj_to_string(&get_stat(&r_stats, "min_date")?),
            mean_date: robj_to_string(&get_stat(&r_stats, "mean_date")?),
            median_date: robj_to_string(&get_stat(&r_stats, "median_date")?),
            max_date: robj_to_string(&get_stat(&r_stats, "max_date")?),
            num_unique: num_unique as i64,
            timezone,
        },
    ))
}

fn robj_to_string(robj: &RObject) -> String {
    let string: Option<String> = unwrap!(robj.clone().try_into(), Err(e) => {
        log::error!("Date stats: Error converting RObject to String: {e}");
        None
    });

    match string {
        Some(s) => s,
        None => {
            log::warn!("Date stats: Expected a string, got NA");
            "NA".to_string()
        },
    }
}

fn call_summary_fn(function: &str, column: SEXP) -> anyhow::Result<RObject> {
    Ok(RFunction::from(function)
        .add(column)
        .call_in(ARK_ENVS.positron_ns)?)
}

macro_rules! impl_summary_stats_conversion {
    ($name:ident, $summary_stats:ident, $comm_type:ty, $display_type:expr) => {
        struct $summary_stats(pub $comm_type);

        impl From<$summary_stats> for ColumnSummaryStats {
            fn from(summary_stats: $summary_stats) -> Self {
                let mut stats = empty_column_summary_stats();
                stats.type_display = $display_type;
                stats.$name = Some(summary_stats.0);
                stats
            }
        }
    };
}

impl_summary_stats_conversion!(
    number_stats,
    SummaryStatsNumber,
    data_explorer_comm::SummaryStatsNumber,
    ColumnDisplayType::Number
);
impl_summary_stats_conversion!(
    string_stats,
    SummaryStatsString,
    data_explorer_comm::SummaryStatsString,
    ColumnDisplayType::String
);
impl_summary_stats_conversion!(
    boolean_stats,
    SummaryStatsBoolean,
    data_explorer_comm::SummaryStatsBoolean,
    ColumnDisplayType::Boolean
);
impl_summary_stats_conversion!(
    date_stats,
    SummaryStatsDate,
    data_explorer_comm::SummaryStatsDate,
    ColumnDisplayType::Date
);
impl_summary_stats_conversion!(
    datetime_stats,
    SummaryStatsDatetime,
    data_explorer_comm::SummaryStatsDatetime,
    ColumnDisplayType::Datetime
);

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
            let expected: ColumnSummaryStats =
                SummaryStatsNumber(data_explorer_comm::SummaryStatsNumber {
                    min_value: "1.00".to_string(),
                    max_value: "5.00".to_string(),
                    mean: "3.00".to_string(),
                    median: "3.00".to_string(),
                    stdev: "1.58".to_string(),
                })
                .into();
            assert_eq!(stats, expected);
        })
    }

    #[test]
    fn test_string_summary() {
        r_test(|| {
            let column = r_parse_eval0("c('a', 'b', 'c', 'd', '')", R_ENVS.global).unwrap();
            let stats =
                summary_stats_(column.sexp, ColumnDisplayType::String, &default_options()).unwrap();
            let expected: ColumnSummaryStats =
                SummaryStatsString(data_explorer_comm::SummaryStatsString {
                    num_empty: 1,
                    num_unique: 5,
                })
                .into();
            assert_eq!(stats, expected);
        })
    }

    #[test]
    fn test_boolean_summary() {
        r_test(|| {
            let column = r_parse_eval0("c(TRUE, FALSE, TRUE, TRUE, NA)", R_ENVS.global).unwrap();
            let stats = summary_stats_(column.sexp, ColumnDisplayType::Boolean, &default_options())
                .unwrap();
            let expected: ColumnSummaryStats =
                SummaryStatsBoolean(data_explorer_comm::SummaryStatsBoolean {
                    true_count: 3,
                    false_count: 1,
                })
                .into();
            assert_eq!(stats, expected);
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
            let expected: ColumnSummaryStats =
                SummaryStatsDate(data_explorer_comm::SummaryStatsDate {
                    min_date: "2021-01-01".to_string(),
                    mean_date: "2021-01-02".to_string(),
                    median_date: "2021-01-02".to_string(),
                    max_date: "2021-01-04".to_string(),
                    num_unique: 5,
                })
                .into();
            assert_eq!(stats, expected);
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
            let expected: ColumnSummaryStats =
                SummaryStatsDatetime(data_explorer_comm::SummaryStatsDatetime {
                    num_unique: 2,
                    min_date: "2015-07-24 23:15:07".to_string(),
                    mean_date: "2015-07-24 23:15:07".to_string(),
                    median_date: "2015-07-24 23:15:07".to_string(),
                    max_date: "2015-07-24 23:15:07".to_string(),
                    timezone: Some("Japan".to_string()),
                })
                .into();
            assert_eq!(stats, expected);
        })
    }
}
