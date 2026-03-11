//
// r-data-explorer.rs
//
// Copyright (C) 2023-2026 by Posit Software, PBC
//
//

use std::cmp;
use std::collections::HashMap;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::data_explorer_comm::ArraySelection;
use amalthea::comm::data_explorer_comm::BackendState;
use amalthea::comm::data_explorer_comm::CodeSyntaxName;
use amalthea::comm::data_explorer_comm::ColumnDisplayType;
use amalthea::comm::data_explorer_comm::ColumnFilter;
use amalthea::comm::data_explorer_comm::ColumnFilterParams;
use amalthea::comm::data_explorer_comm::ColumnFilterType;
use amalthea::comm::data_explorer_comm::ColumnFilterTypeSupportStatus;
use amalthea::comm::data_explorer_comm::ColumnProfileType;
use amalthea::comm::data_explorer_comm::ColumnProfileTypeSupportStatus;
use amalthea::comm::data_explorer_comm::ColumnSchema;
use amalthea::comm::data_explorer_comm::ColumnSelection;
use amalthea::comm::data_explorer_comm::ColumnSortKey;
use amalthea::comm::data_explorer_comm::ColumnValue;
use amalthea::comm::data_explorer_comm::ConvertToCodeFeatures;
use amalthea::comm::data_explorer_comm::ConvertToCodeParams;
use amalthea::comm::data_explorer_comm::ConvertedCode;
use amalthea::comm::data_explorer_comm::DataExplorerBackendReply;
use amalthea::comm::data_explorer_comm::DataExplorerBackendRequest;
use amalthea::comm::data_explorer_comm::DataExplorerFrontendEvent;
use amalthea::comm::data_explorer_comm::ExportDataSelectionFeatures;
use amalthea::comm::data_explorer_comm::ExportDataSelectionParams;
use amalthea::comm::data_explorer_comm::ExportFormat;
use amalthea::comm::data_explorer_comm::ExportedData;
use amalthea::comm::data_explorer_comm::FilterComparisonOp;
use amalthea::comm::data_explorer_comm::FilterResult;
use amalthea::comm::data_explorer_comm::FormatOptions;
use amalthea::comm::data_explorer_comm::GetColumnProfilesFeatures;
use amalthea::comm::data_explorer_comm::GetColumnProfilesParams;
use amalthea::comm::data_explorer_comm::GetDataValuesParams;
use amalthea::comm::data_explorer_comm::GetSchemaParams;
use amalthea::comm::data_explorer_comm::RowFilter;
use amalthea::comm::data_explorer_comm::RowFilterParams;
use amalthea::comm::data_explorer_comm::RowFilterType;
use amalthea::comm::data_explorer_comm::RowFilterTypeSupportStatus;
use amalthea::comm::data_explorer_comm::SearchSchemaFeatures;
use amalthea::comm::data_explorer_comm::SearchSchemaParams;
use amalthea::comm::data_explorer_comm::SearchSchemaResult;
use amalthea::comm::data_explorer_comm::SearchSchemaSortOrder;
use amalthea::comm::data_explorer_comm::SetColumnFiltersFeatures;
use amalthea::comm::data_explorer_comm::SetRowFiltersFeatures;
use amalthea::comm::data_explorer_comm::SetRowFiltersParams;
use amalthea::comm::data_explorer_comm::SetSortColumnsFeatures;
use amalthea::comm::data_explorer_comm::SetSortColumnsParams;
use amalthea::comm::data_explorer_comm::SupportStatus;
use amalthea::comm::data_explorer_comm::SupportedFeatures;
use amalthea::comm::data_explorer_comm::TableData;
use amalthea::comm::data_explorer_comm::TableRowLabels;
use amalthea::comm::data_explorer_comm::TableSchema;
use amalthea::comm::data_explorer_comm::TableSelection;
use amalthea::comm::data_explorer_comm::TableShape;
use amalthea::comm::data_explorer_comm::TextSearchType;
use amalthea::socket::comm::CommOutgoingTx;
use anyhow::anyhow;
use anyhow::bail;
use anyhow::Context;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::r_symbol;
use harp::table_kind;
use harp::tbl_get_column;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use harp::ColumnNames;
use harp::TableKind;
use itertools::Itertools;
use libr::*;
use stdext::result::ResultExt;
use stdext::unwrap;
use tracing::Instrument;

use crate::comm_handler::handle_rpc_request;
use crate::comm_handler::CommHandler;
use crate::comm_handler::CommHandlerContext;
use crate::comm_handler::EnvironmentChanged;
use crate::console::Console;
use crate::data_explorer::column_profile::handle_columns_profiles_requests;
use crate::data_explorer::column_profile::ProcessColumnsProfilesParams;
use crate::data_explorer::convert_to_code;
use crate::data_explorer::export_selection;
use crate::data_explorer::format;
use crate::data_explorer::format::format_string;
use crate::data_explorer::table::Table;
use crate::data_explorer::utils::display_type;
use crate::data_explorer::utils::tbl_subset_with_view_indices;
use crate::modules::ARK_ENVS;
use crate::r_task;
use crate::r_task::RTask;
use crate::variables::variable::WorkspaceVariableDisplayType;

pub const DATA_EXPLORER_COMM_NAME: &str = "positron.dataExplorer";

/// A name/value binding pair in an environment.
///
/// We use this to keep track of the data object that the data viewer is
/// currently viewing; when the binding changes, we update the data viewer
/// accordingly.
pub struct DataObjectEnvInfo {
    pub name: String,
    pub env: RObject,
}

pub(crate) struct DataObjectShape {
    pub columns: Vec<ColumnSchema>,
    pub num_rows: i32,
    pub kind: TableKind,
}

/// The R backend for Positron's Data Explorer.
pub struct RDataExplorer {
    /// The human-readable title of the data viewer.
    title: String,

    /// The data object that the data viewer is currently viewing.
    table: Table,

    /// An optional binding to the environment containing the data object.
    /// This can be omitted for cases wherein the data object isn't in an
    /// environment (e.g. a temporary or unnamed object)
    binding: Option<DataObjectEnvInfo>,

    /// A cache containing the current number of rows and the schema for each
    /// column of the data object.
    shape: DataObjectShape,

    /// A cache containing the current set of sort keys.
    sort_keys: Vec<ColumnSortKey>,

    /// A cache containing the current set of row filters.
    row_filters: Vec<RowFilter>,

    /// A cache containing the current set of column filters
    col_filters: Vec<ColumnFilter>,

    /// The set of sorted row indices, if any sorts are applied. This always
    /// includes all row indices.
    sorted_indices: Option<Vec<i32>>,

    /// The set of filtered row indices, if any filters are applied. These are
    /// the row indices that remain after applying all row filters. They're
    /// sorted in ascending order.
    filtered_indices: Option<Vec<i32>>,

    /// When any sorts or filters are applied, the set of sorted and filtered
    /// row indices. This is the set of row indices that are displayed in the
    /// data viewer.
    view_indices: Option<Vec<i32>>,
}

impl std::fmt::Debug for RDataExplorer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RDataExplorer")
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl RDataExplorer {
    /// Create a new data explorer. Must be called from the R thread.
    pub fn new(
        title: String,
        data: RObject,
        binding: Option<DataObjectEnvInfo>,
    ) -> anyhow::Result<Self> {
        let table = Table::new(data);
        let shape = Self::get_shape(table.get().clone())?;
        Ok(Self {
            title,
            table,
            binding,
            shape,
            sorted_indices: None,
            filtered_indices: None,
            view_indices: None,
            sort_keys: vec![],
            row_filters: vec![],
            col_filters: vec![],
        })
    }

    /// Check the environment bindings for updates to the underlying value
    ///
    /// Returns true if the update was processed; false if the binding has been
    /// removed and the data viewer should be closed.
    fn update(&mut self, ctx: &CommHandlerContext) -> anyhow::Result<bool> {
        // No need to check for updates if we have no binding
        if self.binding.is_none() {
            return Ok(true);
        }

        let binding = self.binding.as_ref().unwrap();
        let env = binding.env.sexp;

        let new = unsafe {
            let sym = r_symbol!(binding.name);
            Rf_findVarInFrame(env, sym)
        };

        let changed = if new == self.table.get().sexp {
            false
        } else {
            self.table.set(RObject::new(new));
            true
        };

        // No change to the value, so we're done
        if !changed {
            return Ok(true);
        }

        // Now we need to check to see if the schema has changed or just a data
        // value. Regenerate the schema.
        //
        // Consider: there may be a cheaper way to test the schema for changes
        // than regenerating it, but it'd be a lot more complicated.
        let new_shape = match Self::get_shape(self.table.get().clone()) {
            Ok(shape) => shape,
            Err(_) => {
                // The most likely cause of this error is that the object is no
                // longer something with a usable shape -- it's been removed or
                // replaced with an object that doesn't work with the data
                // viewer (i.e. is non rectangular)
                return Ok(false);
            },
        };

        // Generate the appropriate event based on whether the schema has
        // changed
        let event = if self.shape.columns != new_shape.columns {
            // Columns changed, so update our cache, and we need to send a
            // schema update event
            self.shape = new_shape;

            // Update row filters to reflect the new schema
            self.row_filters_update()?;

            // Clear precomputed indices
            self.sorted_indices = None;
            self.filtered_indices = None;
            self.view_indices = None;

            // Clear active sort keys
            self.sort_keys.clear();

            // Recompute and apply filters and sorts.
            let (indices, _) = self.row_filters_compute()?;
            self.filtered_indices = indices;
            self.apply_sorts_and_filters();

            DataExplorerFrontendEvent::SchemaUpdate
        } else {
            // The schema didn't change, but the number of rows might have
            // so we need to set the shape to the new_shape
            self.shape = new_shape;

            // Columns didn't change, but the data has. If there are sort
            // keys, we need to sort the rows again to reflect the new data.
            if !self.sort_keys.is_empty() {
                self.sorted_indices = Some(self.sort_rows()?);
            }

            // Recompute and apply filters and sorts.
            let (indices, _) = self.row_filters_compute()?;
            self.filtered_indices = indices;
            self.apply_sorts_and_filters();

            DataExplorerFrontendEvent::DataUpdate
        };

        ctx.outgoing_tx
            .send(CommMsg::Data(serde_json::to_value(event)?))?;
        Ok(true)
    }

    // Marks row_filters as invalid if the column no longer exists
    // If the column still exists, update the column schema of the filter
    // and check if they are still valid.
    // Should be called whenever there's a schema update, ie. when `self.shape` changes.
    fn row_filters_update(&mut self) -> anyhow::Result<()> {
        for rf in self.row_filters.iter_mut() {
            let new_schema = self
                .shape
                .columns
                .iter()
                .find(|c| c.column_name == rf.column_schema.column_name);

            match new_schema {
                Some(schema) => {
                    rf.column_schema = schema.clone();
                    let is_valid = Self::is_valid_filter(rf)?;
                    rf.is_valid = Some(is_valid);
                    rf.error_message = if is_valid {
                        None
                    } else {
                        Some("Unsupported column type for filter".to_string())
                    };
                },
                None => {
                    // the column no longer exists
                    rf.is_valid = Some(false);
                    rf.error_message = Some("Column was removed".to_string());
                },
            };
        }
        Ok(())
    }

    fn handle_rpc(
        &mut self,
        req: DataExplorerBackendRequest,
        ctx: &CommHandlerContext,
    ) -> anyhow::Result<DataExplorerBackendReply> {
        match req {
            DataExplorerBackendRequest::GetSchema(GetSchemaParams { column_indices }) => {
                self.get_schema(column_indices)
            },

            DataExplorerBackendRequest::GetDataValues(GetDataValuesParams {
                columns,
                format_options,
            }) => self.get_data_values(columns, format_options),

            DataExplorerBackendRequest::SetSortColumns(SetSortColumnsParams {
                sort_keys: keys,
            }) => {
                // Save the new sort keys
                self.sort_keys = keys.clone();

                // If there are no sort keys, clear the precomputed sorted
                // indices; otherwise, sort the rows and save the result
                self.sorted_indices = match keys.len() {
                    0 => None,
                    _ => Some(self.sort_rows()?),
                };

                // Apply sorts to the filtered indices to create view indices
                self.apply_sorts_and_filters();

                Ok(DataExplorerBackendReply::SetSortColumnsReply())
            },

            DataExplorerBackendRequest::SetRowFilters(SetRowFiltersParams { filters }) => {
                // Save the new row filters
                self.row_filters = filters;

                // Compute the filtered indices
                let (indices, had_errors) = self.row_filters_compute()?;
                self.filtered_indices = indices;

                // Apply sorts to the filtered indices to create view indices
                self.apply_sorts_and_filters();

                Ok(DataExplorerBackendReply::SetRowFiltersReply({
                    FilterResult {
                        selected_num_rows: match self.filtered_indices {
                            Some(ref indices) => indices.len() as i64,
                            None => self.shape.num_rows as i64,
                        },
                        had_errors,
                    }
                }))
            },

            DataExplorerBackendRequest::GetColumnProfiles(params) => {
                // We respond immediately to this request, but first we launch an
                // R idle task that will compute the column profiles.
                self.launch_get_column_profiles_handler(params, &ctx.outgoing_tx);
                Ok(DataExplorerBackendReply::GetColumnProfilesReply())
            },

            DataExplorerBackendRequest::GetState => self.get_state(),

            DataExplorerBackendRequest::OpenDataset(_) => {
                Err(anyhow!("Data Explorer: Not yet supported"))
            },

            DataExplorerBackendRequest::SearchSchema(params) => self.search_schema(params),

            DataExplorerBackendRequest::SetColumnFilters(_) => {
                Err(anyhow!("Data Explorer: Not yet supported"))
            },

            DataExplorerBackendRequest::GetRowLabels(req) => {
                let row_labels = self.get_row_labels(req.selection, &req.format_options)?;
                Ok(DataExplorerBackendReply::GetRowLabelsReply(
                    TableRowLabels {
                        row_labels: vec![row_labels],
                    },
                ))
            },

            DataExplorerBackendRequest::ExportDataSelection(ExportDataSelectionParams {
                selection,
                format,
            }) => Ok(DataExplorerBackendReply::ExportDataSelectionReply(
                ExportedData {
                    data: self.export_data_selection(selection, format)?,
                    format,
                },
            )),
            DataExplorerBackendRequest::ConvertToCode(params) => Ok(
                DataExplorerBackendReply::ConvertToCodeReply(self.convert_to_code(params)),
            ),
            DataExplorerBackendRequest::SuggestCodeSyntax => Ok(
                DataExplorerBackendReply::SuggestCodeSyntaxReply(self.suggest_code_syntax()),
            ),
        }
    }
}

impl CommHandler for RDataExplorer {
    fn open_metadata(&self) -> serde_json::Value {
        serde_json::json!({ "title": self.title })
    }

    fn handle_msg(&mut self, msg: CommMsg, ctx: &CommHandlerContext) {
        handle_rpc_request(&ctx.outgoing_tx, DATA_EXPLORER_COMM_NAME, msg, |req| {
            self.handle_rpc(req, ctx)
        });
    }

    fn handle_environment(&mut self, event: EnvironmentChanged, ctx: &CommHandlerContext) {
        let EnvironmentChanged::Execution = event else {
            return;
        };
        match self.update(ctx) {
            Ok(true) => {},
            Ok(false) => ctx.close_on_exit(),
            Err(err) => {
                log::error!("Error while checking environment for data viewer update: {err}");
            },
        }
    }
}

impl RDataExplorer {
    pub(crate) fn get_shape(table: RObject) -> anyhow::Result<DataObjectShape> {
        unsafe {
            let table = table.clone();

            let Some(kind) = table_kind(table.sexp) else {
                return Err(anyhow!("Unsupported type for the data viewer"));
            };

            // `DataFrame::n_row()` will materialize duckplyr compact row names, but we
            // are ok with that for the data explorer and don't provide a hook to opt out.
            let (n_row, n_col, column_names) = match kind {
                TableKind::Dataframe => (
                    harp::DataFrame::n_row(table.sexp)?,
                    harp::DataFrame::n_col(table.sexp)?,
                    ColumnNames::from_data_frame(table.sexp)?,
                ),
                TableKind::Matrix => {
                    let (n_row, n_col) = harp::Matrix::dim(table.sexp)?;
                    (n_row, n_col, ColumnNames::from_matrix(table.sexp)?)
                },
            };

            let mut column_schemas = Vec::<ColumnSchema>::new();
            for i in 0..(n_col as isize) {
                let column_name = column_names.get_unchecked(i).unwrap_or_default();

                // TODO: handling for nested data frame columns

                let col = match kind {
                    harp::TableKind::Dataframe => VECTOR_ELT(table.sexp, i),
                    harp::TableKind::Matrix => table.sexp,
                };

                let type_name = WorkspaceVariableDisplayType::from(col, false).display_type;
                let type_display = display_type(col);

                // Get the label attribute if present (for data frames only)
                let column_label = match kind {
                    harp::TableKind::Dataframe => {
                        let col_obj = harp::RObject::view(col);
                        col_obj.get_attribute("label").and_then(|label_obj| {
                            // CharacterVector::new() already checks if it's a STRSXP
                            CharacterVector::new(label_obj.sexp)
                                .ok()
                                .filter(|cv| cv.len() > 0) // Only proceed if non-empty
                                .and_then(|cv| cv.get_unchecked(0))
                                .and_then(|label| {
                                    // Filter out empty strings - treat them as no label
                                    if label.trim().is_empty() {
                                        None
                                    } else {
                                        Some(label.to_string())
                                    }
                                })
                        })
                    },
                    _ => None,
                };

                column_schemas.push(ColumnSchema {
                    column_name,
                    column_label,
                    column_index: i as i64,
                    type_name,
                    type_display,
                    description: None,
                    children: None,
                    precision: None,
                    scale: None,
                    timezone: None,
                    type_size: None,
                });
            }

            Ok(DataObjectShape {
                columns: column_schemas,
                kind,
                num_rows: n_row,
            })
        }
    }

    fn launch_get_column_profiles_handler(
        &self,
        params: GetColumnProfilesParams,
        outgoing_tx: &CommOutgoingTx,
    ) {
        let id = params.callback_id.clone();

        let params = ProcessColumnsProfilesParams {
            table: Table::new(self.table.get().clone()),
            indices: self.filtered_indices.clone(),
            kind: self.shape.kind,
            request: params,
        };
        let outgoing_tx = outgoing_tx.clone();
        r_task::spawn(RTask::idle(async move |_| {
            log::trace!("Processing GetColumnProfile request: {id}");
            handle_columns_profiles_requests(params, outgoing_tx)
                .instrument(tracing::info_span!("get_columns_profile", ns = id))
                .await
                .context("Unable to handle get_columns_profile")
                .log_err();
        }));
    }

    /// Sort the rows of the data object according to the sort keys in
    /// self.sort_keys.
    ///
    /// Returns a vector containing the sorted row indices.
    fn sort_rows(&self) -> anyhow::Result<Vec<i32>> {
        let mut order = RFunction::new("base", "order");

        // Allocate a vector to hold the sort order for each column
        let mut decreasing: Vec<bool> = Vec::new();

        // For each element of self.sort_keys, add an argument to order
        for key in &self.sort_keys {
            // Get the column to sort by
            order.add(tbl_get_column(
                self.table.get().sexp,
                key.column_index as i32,
                self.shape.kind,
            )?);
            decreasing.push(!key.ascending);
        }
        // Add the sort order per column
        order.param("decreasing", RObject::try_from(&decreasing)?);
        order.param("method", RObject::from("radix"));

        // Invoke the order function and return the result
        let result = order.call()?;
        let indices: Vec<i32> = result.try_into()?;
        Ok(indices)
    }

    /// Filter all the rows in the data object according to the row filters in
    /// self.row_filters.
    ///
    /// Returns a tuple containing a vector of all the row indices that pass the filters and
    /// a character vector of errors, where None means no error happened.
    fn filter_rows(&self) -> anyhow::Result<(Vec<i32>, Vec<Option<String>>)> {
        let mut filters: Vec<RObject> = vec![];

        // Shortcut: If there are no row filters, the filtered indices include
        // all row indices.
        if self.row_filters.is_empty() {
            return Ok(((1..=self.shape.num_rows).collect(), vec![]));
        }

        // Convert each filter to an R object by marshaling through the JSON
        // layer.
        //
        // This feels a little weird since the filters were *unmarshaled* from
        // JSON earlier in the RPC stack, but it's the easiest way to create R
        // objects from the filter data without creating an unnecessary
        // intermediate representation.
        for filter in &self.row_filters {
            let filter = serde_json::to_value(filter)?;
            let filter = RObject::try_from(filter)?;
            filters.push(filter);
        }

        // Pass the row filters to R and get the resulting row indices
        let filters = RObject::try_from(filters)?;
        let result: HashMap<String, RObject> = RFunction::new("", ".ps.filter_rows")
            .param("table", self.table.get().sexp)
            .param("row_filters", filters)
            .call_in(ARK_ENVS.positron_ns)?
            .try_into()?;

        // Handle errors that occured in the filters
        let row_indices = match result.get("indices") {
            Some(indices) => Vec::<i32>::try_from(indices.clone())?,
            None => bail!("Unexpected output from .ps.filter_rows. Expected 'indices' field."),
        };

        let errors = match result.get("errors") {
            Some(errors) => Vec::<Option<String>>::try_from(errors.clone())?,
            None => bail!("Unexpected output from .ps.filter_rows. Expected 'errors' field."),
        };

        Ok((row_indices, errors))
    }

    // Compute filtered indices out of the current `row_filters`.
    //
    // Implicitly updates the `row_filters` with validity status and error messages, if they
    // fail during the computation.
    fn row_filters_compute(&mut self) -> anyhow::Result<(Option<Vec<i32>>, Option<bool>)> {
        if self.row_filters.is_empty() {
            return Ok((None, None));
        }

        let (indices, errors) = self.filter_rows()?;
        // this is called for the side-effect of updating the row_filters with validty status and
        // error messages
        let had_errors = Some(self.apply_filter_errors(errors)?);

        Ok((Some(indices), had_errors))
    }

    // Check if a filter is valid by looking at it's type and the type of the column its applied to.
    // Uses logic similar to python side: https://github.com/posit-dev/positron/blob/aafe313a261fd133b9f4a9f87c92bb10dc9966ad/extensions/positron-python/python_files/positron/positron_ipykernel/data_explorer.py#L743-L744
    fn is_valid_filter(filter: &RowFilter) -> anyhow::Result<bool> {
        let display_type = &filter.column_schema.type_display;
        let filter_type = &filter.filter_type;

        let is_compare_supported = |x: &ColumnDisplayType| {
            matches!(
                x,
                ColumnDisplayType::Integer |
                    ColumnDisplayType::Floating |
                    ColumnDisplayType::Decimal |
                    ColumnDisplayType::Date |
                    ColumnDisplayType::Datetime |
                    ColumnDisplayType::Time
            )
        };

        match filter_type {
            RowFilterType::IsEmpty | RowFilterType::NotEmpty | RowFilterType::Search => {
                // String-only filter types
                Ok(display_type == &ColumnDisplayType::String)
            },
            RowFilterType::Compare => {
                if let Some(params) = &filter.params {
                    match params {
                        RowFilterParams::Comparison(comparison) => match comparison.op {
                            FilterComparisonOp::Eq | FilterComparisonOp::NotEq => Ok(true),
                            _ => Ok(is_compare_supported(display_type)),
                        },
                        _ => Err(anyhow!("Missing compare filter params")),
                    }
                } else {
                    Err(anyhow!("Missing compare_params for filter"))
                }
            },
            RowFilterType::Between | RowFilterType::NotBetween => {
                Ok(is_compare_supported(display_type))
            },
            RowFilterType::IsTrue | RowFilterType::IsFalse => {
                Ok(display_type == &ColumnDisplayType::Boolean)
            },
            RowFilterType::IsNull | RowFilterType::NotNull | RowFilterType::SetMembership => {
                // Filters always supported
                Ok(true)
            },
        }
    }

    // Handle errors that occured in the filters
    //
    // This function mutates the `row_filters` attribute to include error messages and validity status.
    fn apply_filter_errors(&mut self, errors: Vec<Option<String>>) -> anyhow::Result<bool> {
        let mut had_errors = false;
        for (i, error) in errors.iter().enumerate() {
            match error {
                None => {
                    self.row_filters[i].is_valid = Some(true);
                },
                Some(error) => {
                    self.row_filters[i].is_valid = Some(false);
                    self.row_filters[i].error_message = Some(error.clone());
                    had_errors = true;
                },
            }
        }
        Ok(had_errors)
    }

    /// Sort the filtered indices according to the sort keys, storing the
    /// result in view_indices.
    fn apply_sorts_and_filters(&mut self) {
        // If there are no filters or sorts, we don't need any view indices
        if self.filtered_indices.is_none() && self.sorted_indices.is_none() {
            self.view_indices = None;
            return;
        }

        // If there are filters but no sorts, the view indices are the filtered
        // indices
        if self.sorted_indices.is_none() {
            self.view_indices = self.filtered_indices.clone();
            return;
        }

        // If there are sorts but no filters, the view indices are the sorted
        // indices
        if self.filtered_indices.is_none() {
            self.view_indices = self.sorted_indices.clone();
            return;
        }

        // There are both sorts and filters, so we need to combine them.
        // self.sorted_indices contains all the indices; self.filtered_indices
        // contains the subset of indices that pass the filters, in ascending
        // order.
        //
        // Derive the set of indices that pass the filters and are sorted
        // according to the sort keys.
        let filtered_indices = self.filtered_indices.as_ref().unwrap();
        let sorted_indices = self.sorted_indices.as_ref().unwrap();
        let mut view_indices = Vec::<i32>::with_capacity(filtered_indices.len());
        for &index in sorted_indices {
            // We can use a binary search here for performance because
            // filtered_indices is already sorted in ascending order.
            if filtered_indices.binary_search(&index).is_ok() {
                view_indices.push(index);
            }
        }
        self.view_indices = Some(view_indices);
    }

    /// Search the schema for columns matching the given filters and sort order.
    ///
    /// - `params`: The search parameters including filters and sort order.
    fn search_schema(
        &self,
        params: SearchSchemaParams,
    ) -> anyhow::Result<DataExplorerBackendReply> {
        let all_columns = &self.shape.columns;

        // Apply column filters to find matching columns using iterator chaining
        let mut matching_indices: Vec<i64> = all_columns
            .iter()
            .enumerate()
            .filter_map(|(index, column)| {
                let column_index = index as i64;

                // Check if column matches all filters
                let matches = params
                    .filters
                    .iter()
                    .all(|filter| self.column_matches_filter(column, filter));

                if matches {
                    Some(column_index)
                } else {
                    None
                }
            })
            .collect();

        // Apply sort order
        match params.sort_order {
            SearchSchemaSortOrder::Original => {
                // matching_indices is already in original order
            },
            SearchSchemaSortOrder::AscendingName => {
                matching_indices.sort_by(|&a, &b| {
                    all_columns[a as usize]
                        .column_name
                        .cmp(&all_columns[b as usize].column_name)
                });
            },
            SearchSchemaSortOrder::DescendingName => {
                matching_indices.sort_by(|&a, &b| {
                    all_columns[b as usize]
                        .column_name
                        .cmp(&all_columns[a as usize].column_name)
                });
            },
            SearchSchemaSortOrder::AscendingType => {
                matching_indices.sort_by(|&a, &b| {
                    all_columns[a as usize]
                        .type_name
                        .to_lowercase()
                        .cmp(&all_columns[b as usize].type_name.to_lowercase())
                });
            },
            SearchSchemaSortOrder::DescendingType => {
                matching_indices.sort_by(|&a, &b| {
                    all_columns[b as usize]
                        .type_name
                        .to_lowercase()
                        .cmp(&all_columns[a as usize].type_name.to_lowercase())
                });
            },
        }

        Ok(DataExplorerBackendReply::SearchSchemaReply(
            SearchSchemaResult {
                matches: matching_indices,
            },
        ))
    }

    /// Check if a column matches a given column filter.
    fn column_matches_filter(&self, column: &ColumnSchema, filter: &ColumnFilter) -> bool {
        match filter.filter_type {
            ColumnFilterType::TextSearch => {
                if let ColumnFilterParams::TextSearch(text_search) = &filter.params {
                    let column_name = if text_search.case_sensitive {
                        column.column_name.to_owned()
                    } else {
                        column.column_name.to_lowercase()
                    };

                    let search_term = if text_search.case_sensitive {
                        text_search.term.to_owned()
                    } else {
                        text_search.term.to_lowercase()
                    };

                    match text_search.search_type {
                        TextSearchType::Contains => column_name.contains(&search_term),
                        TextSearchType::NotContains => !column_name.contains(&search_term),
                        TextSearchType::StartsWith => column_name.starts_with(&search_term),
                        TextSearchType::EndsWith => column_name.ends_with(&search_term),
                        TextSearchType::RegexMatch => {
                            // For regex matching, we use simple string matching as a fallback
                            // A full regex implementation would require additional dependencies
                            column_name.contains(&search_term)
                        },
                    }
                } else {
                    false
                }
            },
            ColumnFilterType::MatchDataTypes => {
                if let ColumnFilterParams::MatchDataTypes(type_filter) = &filter.params {
                    type_filter.display_types.contains(&column.type_display)
                } else {
                    false
                }
            },
        }
    }

    /// Get the schema for a vector of columns in the data object.
    ///
    /// - `column_indices`: The vector of columns in the data object.
    fn get_schema(&self, column_indices: Vec<i64>) -> anyhow::Result<DataExplorerBackendReply> {
        // Get the columns length. (Does Rust optimize loop invariants well?)
        let columns_len = self.shape.columns.len();

        // Gather the column schemas to return.
        let mut columns: Vec<ColumnSchema> = Vec::new();
        for incoming_column_index in column_indices.into_iter().sorted() {
            // Validate that the incoming column index isn't negative.
            if incoming_column_index < 0 {
                return Err(anyhow!(
                    "Column index out of range {0}",
                    incoming_column_index
                ));
            }

            // Get the column index.
            let column_index = incoming_column_index as usize;

            // Break from the loop if the column index exceeds the number of columns.
            if column_index >= columns_len {
                break;
            }

            // Push the column schema.
            columns.push(self.shape.columns[column_index].clone());
        }

        // Return the table schema.
        Ok(DataExplorerBackendReply::GetSchemaReply(TableSchema {
            columns,
        }))
    }

    fn get_state(&self) -> anyhow::Result<DataExplorerBackendReply> {
        let row_names = RFunction::new("base", "row.names")
            .add(self.table.get().clone())
            .call_in(ARK_ENVS.positron_ns)?;

        let state = BackendState {
            display_name: self.title.clone(),
            connected: Some(true),
            error_message: None,
            table_shape: TableShape {
                num_rows: match self.filtered_indices {
                    Some(ref indices) => indices.len() as i64,
                    None => self.shape.num_rows as i64,
                },
                num_columns: self.shape.columns.len() as i64,
            },
            table_unfiltered_shape: TableShape {
                num_rows: self.shape.num_rows as i64,
                num_columns: self.shape.columns.len() as i64,
            },
            row_filters: self.row_filters.clone(),
            column_filters: self.col_filters.clone(),
            sort_keys: self.sort_keys.clone(),
            has_row_labels: !row_names.is_null(),
            supported_features: SupportedFeatures {
                get_column_profiles: GetColumnProfilesFeatures {
                    support_status: SupportStatus::Supported,
                    supported_types: vec![
                        ColumnProfileTypeSupportStatus {
                            profile_type: ColumnProfileType::NullCount,
                            support_status: SupportStatus::Supported,
                        },
                        ColumnProfileTypeSupportStatus {
                            profile_type: ColumnProfileType::SummaryStats,
                            support_status: SupportStatus::Supported,
                        },
                        ColumnProfileTypeSupportStatus {
                            profile_type: ColumnProfileType::SmallHistogram,
                            support_status: SupportStatus::Supported,
                        },
                        ColumnProfileTypeSupportStatus {
                            profile_type: ColumnProfileType::SmallFrequencyTable,
                            support_status: SupportStatus::Supported,
                        },
                        ColumnProfileTypeSupportStatus {
                            profile_type: ColumnProfileType::LargeHistogram,
                            support_status: SupportStatus::Supported,
                        },
                        ColumnProfileTypeSupportStatus {
                            profile_type: ColumnProfileType::LargeFrequencyTable,
                            support_status: SupportStatus::Supported,
                        },
                    ],
                },
                search_schema: SearchSchemaFeatures {
                    support_status: SupportStatus::Supported,
                    supported_types: vec![
                        ColumnFilterTypeSupportStatus {
                            column_filter_type: ColumnFilterType::TextSearch,
                            support_status: SupportStatus::Supported,
                        },
                        ColumnFilterTypeSupportStatus {
                            column_filter_type: ColumnFilterType::MatchDataTypes,
                            support_status: SupportStatus::Supported,
                        },
                    ],
                },
                set_row_filters: SetRowFiltersFeatures {
                    support_status: SupportStatus::Supported,
                    supported_types: [
                        RowFilterType::Between,
                        RowFilterType::Compare,
                        RowFilterType::IsEmpty,
                        RowFilterType::IsFalse,
                        RowFilterType::IsNull,
                        RowFilterType::IsTrue,
                        RowFilterType::NotBetween,
                        RowFilterType::NotEmpty,
                        RowFilterType::NotNull,
                        RowFilterType::Search,
                        RowFilterType::SetMembership,
                    ]
                    .iter()
                    .map(|row_filter_type| RowFilterTypeSupportStatus {
                        row_filter_type: *row_filter_type,
                        support_status: SupportStatus::Supported,
                    })
                    .collect(),
                    // Temporarily disabled for https://github.com/posit-dev/positron/issues/3489
                    // on 6/11/2024. This will be enabled again when the UI has been reworked to
                    // support grouping.
                    supports_conditions: SupportStatus::Unsupported,
                },
                set_column_filters: SetColumnFiltersFeatures {
                    support_status: SupportStatus::Unsupported,
                    supported_types: vec![],
                },
                set_sort_columns: SetSortColumnsFeatures {
                    support_status: SupportStatus::Supported,
                },
                export_data_selection: ExportDataSelectionFeatures {
                    support_status: SupportStatus::Supported,
                    supported_formats: vec![
                        ExportFormat::Csv,
                        ExportFormat::Tsv,
                        ExportFormat::Html,
                    ],
                },
                convert_to_code: ConvertToCodeFeatures {
                    support_status: SupportStatus::Supported,
                    code_syntaxes: Some(vec![CodeSyntaxName {
                        code_syntax_name: "dplyr".into(),
                    }]),
                },
            },
        };
        Ok(DataExplorerBackendReply::GetStateReply(state))
    }

    fn get_data_values(
        &self,
        columns: Vec<ColumnSelection>,
        format_options: FormatOptions,
    ) -> anyhow::Result<DataExplorerBackendReply> {
        let mut column_data: Vec<Vec<ColumnValue>> = Vec::with_capacity(columns.len());
        for selection in columns {
            let tbl = tbl_subset_with_view_indices(
                self.table.get().sexp,
                &self.view_indices,
                Some(self.get_row_selection_indices(selection.spec)),
                Some(vec![selection.column_index]),
            )?;

            // The column will be always at index 0 because we already selected a single column above.
            let column = tbl_get_column(tbl.sexp, 0, self.shape.kind)?;
            let formatted = format::format_column(column.sexp, &format_options);
            column_data.push(formatted.clone());
        }

        let response = TableData {
            columns: column_data,
        };

        Ok(DataExplorerBackendReply::GetDataValuesReply(response))
    }

    fn get_row_labels(
        &self,
        selection: ArraySelection,
        format_options: &FormatOptions,
    ) -> anyhow::Result<Vec<String>> {
        let tbl = tbl_subset_with_view_indices(
            self.table.get().sexp,
            &self.view_indices,
            Some(self.get_row_selection_indices(selection)),
            Some(vec![]), // Use empty vec, because we only need the row names.
        )?;

        let row_names = RFunction::new("base", "row.names")
            .add(tbl)
            .call_in(ARK_ENVS.positron_ns)?;

        match row_names.kind() {
            STRSXP => {
                let labels = format_string(row_names.sexp, format_options);
                Ok(labels)
            },
            _ => Err(anyhow!(
                "`row.names` should be strings, got {:?}",
                row_names.kind()
            )),
        }
    }

    // Given an ArraySelection, this materializes the indices that will actually be used.
    // Also does some sanity checks to avoid OOB access.
    fn get_row_selection_indices(&self, selection: ArraySelection) -> Vec<i64> {
        let num_view_rows = match self.view_indices {
            Some(ref indices) => indices.len() as i32,
            None => self.shape.num_rows,
        } as i64;

        // Returns the indices that will be collected
        match selection {
            ArraySelection::SelectRange(range) => {
                let lower_bound = cmp::min(range.first_index, num_view_rows);
                let upper_bound = cmp::min(range.last_index + 1, num_view_rows);
                (lower_bound..upper_bound).collect()
            },
            ArraySelection::SelectIndices(indices) => indices
                .indices
                .into_iter()
                .filter(|v| *v < num_view_rows)
                .collect(),
        }
    }

    fn export_data_selection(
        &self,
        selection: TableSelection,
        format: ExportFormat,
    ) -> anyhow::Result<String> {
        export_selection::export_selection(
            self.table.get().sexp,
            &self.view_indices,
            selection,
            format,
        )
    }

    /// Suggest code syntax for code conversion
    ///
    /// Returns the preferred code syntax for converting data explorer operations to code.
    fn suggest_code_syntax(&self) -> CodeSyntaxName {
        convert_to_code::suggest_code_syntax()
    }

    /// Convert the current data view state to code
    ///
    /// Takes the current filters, sort keys, and other parameters and converts them
    /// to executable code that can reproduce the current data view.
    fn convert_to_code(&self, params: ConvertToCodeParams) -> ConvertedCode {
        // Get object name if available, fallback to title
        let object_name = self
            .binding
            .as_ref()
            .map(|b| b.name.as_str())
            .or(Some(self.title.as_str()));

        // Resolve column names for sort keys using the same pattern as `sort_rows()`
        let resolved_sort_keys: Vec<convert_to_code::ResolvedSortKey> = params
            .sort_keys
            .iter()
            .filter_map(|sort_key| {
                // Get column schema from index, similar to existing sort implementation
                let column_index = sort_key.column_index as usize;
                if column_index < self.shape.columns.len() {
                    Some(convert_to_code::ResolvedSortKey {
                        column_name: self.shape.columns[column_index].column_name.clone(),
                        ascending: sort_key.ascending,
                    })
                } else {
                    // Invalid column index - skip this sort key
                    None
                }
            })
            .collect();

        // Call the conversion function with resolved sort keys
        convert_to_code::convert_to_code(params, object_name, &resolved_sort_keys)
    }
}

/// Open an R object in the data viewer.
///
/// This function is called from the R side to open an R object in the data viewer.
///
/// # Parameters
/// - `x`: The R object to open in the data viewer.
/// - `title`: The title of the data viewer.
/// - `var`: The name of the variable containing the R object in its
///   environment; optional.
/// - `env`: The environment containing the R object; optional.
#[harp::register]
pub unsafe extern "C-unwind" fn ps_view_data_frame(
    x: SEXP,
    title: SEXP,
    var: SEXP,
    env: SEXP,
) -> anyhow::Result<SEXP> {
    let x = RObject::new(x);

    let title = RObject::new(title);
    let title = unwrap!(String::try_from(title), Err(_) => "".to_string());

    // If an environment is provided, watch the variable in the environment
    let env_info = if env != R_NilValue {
        let var_obj = RObject::new(var);
        // Attempt to convert the variable name to a string
        match String::try_from(var_obj.clone()) {
            Ok(var_name) => Some(DataObjectEnvInfo {
                name: var_name,
                env: RObject::new(env),
            }),
            Err(_) => {
                // If the variable name can't be converted to a string, don't
                // watch the variable.
                log::warn!(
                    "Attempt to watch variable in environment failed: {:?} not a string",
                    var_obj
                );
                None
            },
        }
    } else {
        None
    };

    let explorer = RDataExplorer::new(title, x, env_info)?;
    Console::get_mut().comm_register(DATA_EXPLORER_COMM_NAME, Box::new(explorer))?;

    Ok(R_NilValue)
}
