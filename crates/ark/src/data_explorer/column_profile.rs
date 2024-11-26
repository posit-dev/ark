//
// column_profile.rs
//
// Copyright (C) 2024 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::data_explorer_comm::ColumnFrequencyTable;
use amalthea::comm::data_explorer_comm::ColumnHistogram;
use amalthea::comm::data_explorer_comm::ColumnProfileParams;
use amalthea::comm::data_explorer_comm::ColumnProfileRequest;
use amalthea::comm::data_explorer_comm::ColumnProfileResult;
use amalthea::comm::data_explorer_comm::ColumnProfileSpec;
use amalthea::comm::data_explorer_comm::ColumnProfileType;
use amalthea::comm::data_explorer_comm::ColumnSummaryStats;
use amalthea::comm::data_explorer_comm::DataExplorerFrontendEvent;
use amalthea::comm::data_explorer_comm::FormatOptions;
use amalthea::comm::data_explorer_comm::GetColumnProfilesParams;
use amalthea::comm::data_explorer_comm::ReturnColumnProfilesParams;
use amalthea::socket::comm::CommSocket;
use anyhow::anyhow;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::tbl_get_column;
use harp::RObject;
use harp::TableKind;
use stdext::unwrap;

use crate::data_explorer::histogram;
use crate::data_explorer::summary_stats::summary_stats;
use crate::data_explorer::table::Table;
use crate::data_explorer::utils::display_type;
use crate::modules::ARK_ENVS;

pub struct ProcessColumnsProfilesParams {
    pub table: Table,
    pub indices: Option<Vec<i32>>,
    pub kind: TableKind,
    pub request: GetColumnProfilesParams,
}

pub async fn handle_columns_profiles_requests(
    params: ProcessColumnsProfilesParams,
    comm: CommSocket,
) -> anyhow::Result<()> {
    let callback_id = params.request.callback_id;
    let n_profiles = params.request.profiles.len();

    let profiles = process_columns_profiles_requests(
        params.table,
        params.indices,
        params.kind,
        params.request.profiles,
        params.request.format_options,
    )
    .await
    .unwrap_or_else(|e| {
        // In case something goes wrong while computing the profiles, we send
        // an empty response. Ideally, we would have a way to comunicate an that
        // an error happened but it's not implemented yet.
        log::error!("Error while producing profiles: {e}");
        std::iter::repeat(empty_column_profile_result())
            .take(n_profiles)
            .collect()
    });

    let event = DataExplorerFrontendEvent::ReturnColumnProfiles(ReturnColumnProfilesParams {
        callback_id,
        profiles,
        error_message: None,
    });

    let json_event = serde_json::to_value(event)?;
    comm.outgoing_tx.send(CommMsg::Data(json_event))?;
    Ok(())
}

async fn process_columns_profiles_requests(
    table: Table,
    indices: Option<Vec<i32>>,
    kind: TableKind,
    profiles: Vec<ColumnProfileRequest>,
    format_options: FormatOptions,
) -> anyhow::Result<Vec<ColumnProfileResult>> {
    // This is an R thread, so we can actually get the data frame.
    // If it fails we quickly return an empty result set and end the task.
    // This might happen if the task was spawned but the data explorer windows
    // was later closed, before the task actually executed.
    let data = table.get()?;
    let mut results: Vec<ColumnProfileResult> = Vec::with_capacity(profiles.len());

    for profile in profiles.into_iter() {
        log::trace!("Processing column!");
        results.push(
            profile_column(
                data.clone(),
                indices.clone(),
                profile,
                &format_options,
                kind,
            )
            .await,
        );
        // Yield to the idle event loop
        tokio::task::yield_now().await;
    }

    Ok(results)
}

// This function does not return a Result because it must handle still handle other profile types
// if one of them fails. Thus it needs to gracefully handle the errors that might have resulted
// here.
// It's an async function just because we want to yield to R between each profile type.
async fn profile_column(
    table: RObject,
    filtered_indices: Option<Vec<i32>>,
    request: ColumnProfileRequest,
    format_options: &FormatOptions,
    kind: TableKind,
) -> ColumnProfileResult {
    let mut output = empty_column_profile_result();

    let filtered_column = unwrap!(tbl_get_filtered_column(
        &table,
        request.column_index,
        &filtered_indices,
        kind,
    ), Err(e) =>  {
        // In the case something goes wrong here we log the error and return an empty output.
        // This might still work for the other columns in the request.
        log::error!("Error applying filter indices for column: {}. Err: {e}", request.column_index);
        return output;
    });

    for profile_req in request.profiles {
        match profile_req.profile_type {
            ColumnProfileType::NullCount => {
                output.null_count = profile_null_count(filtered_column.clone())
                    .map_err(|err| {
                        log::error!(
                            "Error getting summary stats for column {}: {}",
                            request.column_index,
                            err
                        );
                    })
                    .ok();
            },
            ColumnProfileType::SummaryStats => {
                output.summary_stats =
                    profile_summary_stats(filtered_column.clone(), format_options)
                        .map_err(|err| {
                            log::error!(
                                "Error getting null count for column {}: {}",
                                request.column_index,
                                err
                            );
                        })
                        .ok()
            },
            ColumnProfileType::SmallHistogram | ColumnProfileType::LargeHistogram => {
                let histogram =
                    profile_histogram(filtered_column.clone(), format_options, &profile_req)
                        .map_err(|err| {
                            log::error!(
                                "Error getting histogram for column {}: {}",
                                request.column_index,
                                err
                            );
                        })
                        .ok();

                match profile_req.profile_type {
                    ColumnProfileType::SmallHistogram => {
                        output.small_histogram = histogram;
                    },
                    ColumnProfileType::LargeHistogram => {
                        output.large_histogram = histogram;
                    },
                    _ => {
                        // This is technically unreachable!(), but not worth panicking if
                        // this happens.
                        log::warn!("Unreachable");
                    },
                }
            },
            ColumnProfileType::SmallFrequencyTable | ColumnProfileType::LargeFrequencyTable => {
                let frequency_table =
                    profile_frequency_table(filtered_column.clone(), format_options, &profile_req)
                        .map_err(|err| {
                            log::error!(
                                "Error getting frequency table for column {}: {}",
                                request.column_index,
                                err
                            );
                        })
                        .ok();

                match profile_req.profile_type {
                    ColumnProfileType::SmallFrequencyTable => {
                        output.small_frequency_table = frequency_table;
                    },
                    ColumnProfileType::LargeFrequencyTable => {
                        output.large_frequency_table = frequency_table;
                    },
                    _ => {
                        // This is technically unreachable!(), but not worth panicking if
                        // this happens.
                        log::warn!("Unreachable. Unknown profile type.")
                    },
                }
            },
        };

        // Yield to the R console loop
        tokio::task::yield_now().await;
    }
    output
}

pub fn empty_column_profile_result() -> ColumnProfileResult {
    ColumnProfileResult {
        null_count: None,
        summary_stats: None,
        small_histogram: None,
        small_frequency_table: None,
        large_histogram: None,
        large_frequency_table: None,
    }
}

fn profile_frequency_table(
    column: RObject,
    format_options: &FormatOptions,
    profile_spec: &ColumnProfileSpec,
) -> anyhow::Result<ColumnFrequencyTable> {
    let params = match &profile_spec.params {
        None => return Err(anyhow!("Missing parameters for the frequency table")),
        Some(par) => match par {
            ColumnProfileParams::SmallFrequencyTable(p) => p,
            ColumnProfileParams::LargeFrequencyTable(p) => p,
            _ => return Err(anyhow!("Wrong type of parameters for the frequency table.")),
        },
    };
    let frequency_table =
        histogram::profile_frequency_table(column.sexp, &params, &format_options)?;
    Ok(frequency_table)
}

fn profile_histogram(
    column: RObject,
    format_options: &FormatOptions,
    profile_spec: &ColumnProfileSpec,
) -> anyhow::Result<ColumnHistogram> {
    let params = match &profile_spec.params {
        None => return Err(anyhow!("Missing parameters for the histogram")),
        Some(par) => match par {
            ColumnProfileParams::SmallHistogram(p) => p,
            ColumnProfileParams::LargeHistogram(p) => p,
            _ => return Err(anyhow!("Wrong type of parameters for the histogram.")),
        },
    };
    let histogram = histogram::profile_histogram(column.sexp, &params, &format_options)?;
    Ok(histogram)
}

fn profile_summary_stats(
    column: RObject,
    format_options: &FormatOptions,
) -> anyhow::Result<ColumnSummaryStats> {
    let dtype = display_type(column.sexp);
    Ok(summary_stats(column.sexp, dtype, format_options)?)
}

/// Counts the number of nulls in a column. As the intent is to provide an
/// idea of how complete the data is, NA values are considered to be null
/// for the purposes of these stats.
///
/// Expects data to be filtered by the view indices.
///
/// - `column_index`: The index of the column to count nulls in; 0-based.
fn profile_null_count(column: RObject) -> anyhow::Result<i64> {
    // Compute the number of nulls in the column
    let result: i32 = RFunction::new("", ".ps.null_count")
        .param("column", column)
        .call_in(ARK_ENVS.positron_ns)?
        .try_into()?;

    // Return the count of nulls and NA values
    Ok(result.try_into()?)
}

fn tbl_get_filtered_column(
    x: &RObject,
    column_index: i64,
    indices: &Option<Vec<i32>>,
    kind: TableKind,
) -> anyhow::Result<RObject> {
    let column = tbl_get_column(x.sexp, column_index as i32, kind)?;

    Ok(match &indices {
        Some(indices) => RFunction::from("col_filter_indices")
            .add(column)
            .add(RObject::try_from(indices)?)
            .call_in(ARK_ENVS.positron_ns)?,
        None => column,
    })
}
