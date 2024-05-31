// @generated

/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from data_explorer.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// Result in Methods
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SearchSchemaResult {
	/// A schema containing matching columns up to the max_results limit
	pub matches: Option<TableSchema>,

	/// The total number of columns matching the search term
	pub total_num_matches: i64
}

/// Exported result
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExportedData {
	/// Exported data as a string suitable for copy and paste
	pub data: String,

	/// The exported data format
	pub format: ExportFormat
}

/// The result of applying filters to a table
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FilterResult {
	/// Number of rows in table after applying filters
	pub selected_num_rows: i64,

	/// Flag indicating if there were errors in evaluation
	pub had_errors: Option<bool>
}

/// The current backend state for the data explorer
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BackendState {
	/// Variable name or other string to display for tab name in UI
	pub display_name: String,

	/// Number of rows and columns in table with filters applied
	pub table_shape: TableShape,

	/// Number of rows and columns in table without any filters applied
	pub table_unfiltered_shape: TableShape,

	/// The set of currently applied row filters
	pub row_filters: Vec<RowFilter>,

	/// The set of currently applied sorts
	pub sort_keys: Vec<ColumnSortKey>,

	/// The features currently supported by the backend instance
	pub supported_features: SupportedFeatures
}

/// Schema for a column in a table
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnSchema {
	/// Name of column as UTF-8 string
	pub column_name: String,

	/// The position of the column within the schema
	pub column_index: i64,

	/// Exact name of data type used by underlying table
	pub type_name: String,

	/// Canonical Positron display name of data type
	pub type_display: ColumnDisplayType,

	/// Column annotation / description
	pub description: Option<String>,

	/// Schema of nested child types
	pub children: Option<Vec<ColumnSchema>>,

	/// Precision for decimal types
	pub precision: Option<i64>,

	/// Scale for decimal types
	pub scale: Option<i64>,

	/// Time zone for timestamp with time zone
	pub timezone: Option<String>,

	/// Size parameter for fixed-size types (list, binary)
	pub type_size: Option<i64>
}

/// Table values formatted as strings
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TableData {
	/// The columns of data
	pub columns: Vec<Vec<ColumnValue>>,

	/// Zero or more arrays of row labels
	pub row_labels: Option<Vec<Vec<String>>>
}

/// Formatting options for returning data values as strings
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FormatOptions {
	/// Fixed number of decimal places to display for numbers over 1, or in
	/// scientific notation
	pub large_num_digits: i64,

	/// Fixed number of decimal places to display for small numbers, and to
	/// determine lower threshold for switching to scientific notation
	pub small_num_digits: i64,

	/// Maximum number of integral digits to display before switching to
	/// scientific notation
	pub max_integral_digits: i64,

	/// Thousands separator string
	pub thousands_sep: Option<String>
}

/// The schema for a table-like object
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TableSchema {
	/// Schema for each column in the table
	pub columns: Vec<ColumnSchema>
}

/// Provides number of rows and columns in a table
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TableShape {
	/// Numbers of rows in the table
	pub num_rows: i64,

	/// Number of columns in the table
	pub num_columns: i64
}

/// Specifies a table row filter based on a single column's values
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RowFilter {
	/// Unique identifier for this filter
	pub filter_id: String,

	/// Type of row filter to apply
	pub filter_type: RowFilterType,

	/// Column to apply filter to
	pub column_schema: ColumnSchema,

	/// The binary condition to use to combine with preceding row filters
	pub condition: RowFilterCondition,

	/// Whether the filter is valid and supported by the backend, if undefined
	/// then true
	pub is_valid: Option<bool>,

	/// Optional error message when the filter is invalid
	pub error_message: Option<String>,

	/// Parameters for the 'between' and 'not_between' filter types
	pub between_params: Option<BetweenFilterParams>,

	/// Parameters for the 'compare' filter type
	pub compare_params: Option<CompareFilterParams>,

	/// Parameters for the 'search' filter type
	pub search_params: Option<SearchFilterParams>,

	/// Parameters for the 'set_membership' filter type
	pub set_membership_params: Option<SetMembershipFilterParams>
}

/// Parameters for the 'between' and 'not_between' filter types
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BetweenFilterParams {
	/// The lower limit for filtering
	pub left_value: String,

	/// The upper limit for filtering
	pub right_value: String
}

/// Parameters for the 'compare' filter type
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CompareFilterParams {
	/// String representation of a binary comparison
	pub op: CompareFilterParamsOp,

	/// A stringified column value for a comparison filter
	pub value: String
}

/// Parameters for the 'set_membership' filter type
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SetMembershipFilterParams {
	/// Array of column values for a set membership filter
	pub values: Vec<String>,

	/// Filter by including only values passed (true) or excluding (false)
	pub inclusive: bool
}

/// Parameters for the 'search' filter type
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SearchFilterParams {
	/// Type of search to perform
	pub search_type: SearchFilterType,

	/// String value/regex to search for in stringified data
	pub term: String,

	/// If true, do a case-sensitive search, otherwise case-insensitive
	pub case_sensitive: bool
}

/// A single column profile request
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnProfileRequest {
	/// The ordinal column index to profile
	pub column_index: i64,

	/// The type of analytical column profile
	pub profile_type: ColumnProfileType
}

/// Result of computing column profile
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnProfileResult {
	/// Result from null_count request
	pub null_count: Option<i64>,

	/// Results from summary_stats request
	pub summary_stats: Option<ColumnSummaryStats>,

	/// Results from summary_stats request
	pub histogram: Option<ColumnHistogram>,

	/// Results from frequency_table request
	pub frequency_table: Option<ColumnFrequencyTable>
}

/// Profile result containing summary stats for a column based on the data
/// type
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnSummaryStats {
	/// Canonical Positron display name of data type
	pub type_display: ColumnDisplayType,

	/// Statistics for a numeric data type
	pub number_stats: Option<SummaryStatsNumber>,

	/// Statistics for a string-like data type
	pub string_stats: Option<SummaryStatsString>,

	/// Statistics for a boolean data type
	pub boolean_stats: Option<SummaryStatsBoolean>,

	/// Statistics for a date data type
	pub date_stats: Option<SummaryStatsDate>,

	/// Statistics for a datetime data type
	pub datetime_stats: Option<SummaryStatsDatetime>
}

/// SummaryStatsNumber in Schemas
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SummaryStatsNumber {
	/// Minimum value as string
	pub min_value: String,

	/// Maximum value as string
	pub max_value: String,

	/// Average value as string
	pub mean: String,

	/// Sample median (50% value) value as string
	pub median: String,

	/// Sample standard deviation as a string
	pub stdev: String
}

/// SummaryStatsBoolean in Schemas
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SummaryStatsBoolean {
	/// The number of non-null true values
	pub true_count: i64,

	/// The number of non-null false values
	pub false_count: i64
}

/// SummaryStatsString in Schemas
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SummaryStatsString {
	/// The number of empty / length-zero values
	pub num_empty: i64,

	/// The exact number of distinct values
	pub num_unique: i64
}

/// SummaryStatsDate in Schemas
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SummaryStatsDate {
	/// The exact number of distinct values
	pub num_unique: i64,

	/// Minimum date value as string
	pub min_date: String,

	/// Average date value as string
	pub mean_date: String,

	/// Sample median (50% value) date value as string
	pub median_date: String,

	/// Maximum date value as string
	pub max_date: String
}

/// SummaryStatsDatetime in Schemas
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SummaryStatsDatetime {
	/// The exact number of distinct values
	pub num_unique: i64,

	/// Minimum date value as string
	pub min_date: String,

	/// Average date value as string
	pub mean_date: String,

	/// Sample median (50% value) date value as string
	pub median_date: String,

	/// Maximum date value as string
	pub max_date: String,

	/// Time zone for timestamp with time zone
	pub timezone: Option<String>
}

/// Result from a histogram profile request
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnHistogram {
	/// Absolute count of values in each histogram bin
	pub bin_sizes: Vec<i64>,

	/// Absolute floating-point width of a histogram bin
	pub bin_width: f64
}

/// Result from a frequency_table profile request
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnFrequencyTable {
	/// Counts of distinct values in column
	pub counts: Vec<ColumnFrequencyTableItem>,

	/// Number of other values not accounted for in counts. May be 0
	pub other_count: i64
}

/// Entry in a column's frequency table
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnFrequencyTableItem {
	/// Stringified value
	pub value: String,

	/// Number of occurrences of value
	pub count: i64
}

/// An exact or approximate quantile value from a column
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnQuantileValue {
	/// Quantile number (percentile). E.g. 1 for 1%, 50 for median
	pub q: f64,

	/// Stringified quantile value
	pub value: String,

	/// Whether value is exact or approximate (computed from binned data or
	/// sketches)
	pub exact: bool
}

/// Specifies a column to sort by
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnSortKey {
	/// Column index to sort by
	pub column_index: i64,

	/// Sort order, ascending (true) or descending (false)
	pub ascending: bool
}

/// For each field, returns flags indicating supported features
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SupportedFeatures {
	/// Support for 'search_schema' RPC and its features
	pub search_schema: SearchSchemaFeatures,

	/// Support for 'set_row_filters' RPC and its features
	pub set_row_filters: SetRowFiltersFeatures,

	/// Support for 'get_column_profiles' RPC and its features
	pub get_column_profiles: GetColumnProfilesFeatures
}

/// Feature flags for 'search_schema' RPC
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SearchSchemaFeatures {
	/// Whether this RPC method is supported at all
	pub supported: bool
}

/// Feature flags for 'set_row_filters' RPC
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SetRowFiltersFeatures {
	/// Whether this RPC method is supported at all
	pub supported: bool,

	/// Whether AND/OR filter conditions are supported
	pub supports_conditions: bool,

	/// A list of supported types
	pub supported_types: Vec<RowFilterType>
}

/// Feature flags for 'get_column_profiles' RPC
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GetColumnProfilesFeatures {
	/// Whether this RPC method is supported at all
	pub supported: bool,

	/// A list of supported types
	pub supported_types: Vec<ColumnProfileType>
}

/// A selection on the data grid, for copying to the clipboard or other
/// actions
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DataSelection {
	/// Type of selection
	pub kind: DataSelectionKind,

	/// A union of selection types
	pub selection: Selection
}

/// A selection that contains a single data cell
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DataSelectionSingleCell {
	/// The selected row index
	pub row_index: i64,

	/// The selected column index
	pub column_index: i64
}

/// A selection that contains a rectangular range of data cells
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DataSelectionCellRange {
	/// The starting selected row index (inclusive)
	pub first_row_index: i64,

	/// The final selected row index (inclusive)
	pub last_row_index: i64,

	/// The starting selected column index (inclusive)
	pub first_column_index: i64,

	/// The final selected column index (inclusive)
	pub last_column_index: i64
}

/// A contiguous selection bounded by inclusive start and end indices
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DataSelectionRange {
	/// The starting selected index (inclusive)
	pub first_index: i64,

	/// The final selected index (inclusive)
	pub last_index: i64
}

/// A selection defined by a sequence of indices to include
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DataSelectionIndices {
	/// The selected indices
	pub indices: Vec<i64>
}

/// Possible values for ColumnDisplayType
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ColumnDisplayType {
	#[serde(rename = "number")]
	Number,

	#[serde(rename = "boolean")]
	Boolean,

	#[serde(rename = "string")]
	String,

	#[serde(rename = "date")]
	Date,

	#[serde(rename = "datetime")]
	Datetime,

	#[serde(rename = "time")]
	Time,

	#[serde(rename = "array")]
	Array,

	#[serde(rename = "struct")]
	Struct,

	#[serde(rename = "unknown")]
	Unknown
}

/// Possible values for Condition in RowFilter
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum RowFilterCondition {
	#[serde(rename = "and")]
	And,

	#[serde(rename = "or")]
	Or
}

/// Possible values for RowFilterType
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum RowFilterType {
	#[serde(rename = "between")]
	Between,

	#[serde(rename = "compare")]
	Compare,

	#[serde(rename = "is_empty")]
	IsEmpty,

	#[serde(rename = "is_false")]
	IsFalse,

	#[serde(rename = "is_null")]
	IsNull,

	#[serde(rename = "is_true")]
	IsTrue,

	#[serde(rename = "not_between")]
	NotBetween,

	#[serde(rename = "not_empty")]
	NotEmpty,

	#[serde(rename = "not_null")]
	NotNull,

	#[serde(rename = "search")]
	Search,

	#[serde(rename = "set_membership")]
	SetMembership
}

/// Possible values for Op in CompareFilterParams
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum CompareFilterParamsOp {
	#[serde(rename = "=")]
	Eq,

	#[serde(rename = "!=")]
	NotEq,

	#[serde(rename = "<")]
	Lt,

	#[serde(rename = "<=")]
	LtEq,

	#[serde(rename = ">")]
	Gt,

	#[serde(rename = ">=")]
	GtEq
}

/// Possible values for SearchFilterType
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum SearchFilterType {
	#[serde(rename = "contains")]
	Contains,

	#[serde(rename = "starts_with")]
	StartsWith,

	#[serde(rename = "ends_with")]
	EndsWith,

	#[serde(rename = "regex_match")]
	RegexMatch
}

/// Possible values for ColumnProfileType
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ColumnProfileType {
	#[serde(rename = "null_count")]
	NullCount,

	#[serde(rename = "summary_stats")]
	SummaryStats,

	#[serde(rename = "frequency_table")]
	FrequencyTable,

	#[serde(rename = "histogram")]
	Histogram
}

/// Possible values for Kind in DataSelection
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum DataSelectionKind {
	#[serde(rename = "single_cell")]
	SingleCell,

	#[serde(rename = "cell_range")]
	CellRange,

	#[serde(rename = "column_range")]
	ColumnRange,

	#[serde(rename = "row_range")]
	RowRange,

	#[serde(rename = "column_indices")]
	ColumnIndices,

	#[serde(rename = "row_indices")]
	RowIndices
}

/// Possible values for ExportFormat
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ExportFormat {
	#[serde(rename = "csv")]
	Csv,

	#[serde(rename = "tsv")]
	Tsv,

	#[serde(rename = "html")]
	Html
}

/// Union type ColumnValue
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum ColumnValue {
	SpecialValueCode(i64),

	FormattedValue(String)
}

/// Union type Selection in Properties
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Selection {
	SingleCell(DataSelectionSingleCell),

	CellRange(DataSelectionCellRange),

	IndexRange(DataSelectionRange),

	Indices(DataSelectionIndices)
}

/// Parameters for the GetSchema method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GetSchemaParams {
	/// First column schema to fetch (inclusive)
	pub start_index: i64,

	/// Number of column schemas to fetch from start index. May extend beyond
	/// end of table
	pub num_columns: i64,
}

/// Parameters for the SearchSchema method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SearchSchemaParams {
	/// Substring to match for (currently case insensitive)
	pub search_term: String,

	/// Index (starting from zero) of first result to fetch
	pub start_index: i64,

	/// Maximum number of resulting column schemas to fetch from the start
	/// index
	pub max_results: i64,
}

/// Parameters for the GetDataValues method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GetDataValuesParams {
	/// First row to fetch (inclusive)
	pub row_start_index: i64,

	/// Number of rows to fetch from start index. May extend beyond end of
	/// table
	pub num_rows: i64,

	/// Indices to select, which can be a sequential, sparse, or random
	/// selection
	pub column_indices: Vec<i64>,

	/// Formatting options for returning data values as strings
	pub format_options: FormatOptions,
}

/// Parameters for the ExportDataSelection method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExportDataSelectionParams {
	/// The data selection
	pub selection: DataSelection,

	/// Result string format
	pub format: ExportFormat,
}

/// Parameters for the SetRowFilters method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SetRowFiltersParams {
	/// Zero or more filters to apply
	pub filters: Vec<RowFilter>,
}

/// Parameters for the SetSortColumns method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SetSortColumnsParams {
	/// Pass zero or more keys to sort by. Clears any existing keys
	pub sort_keys: Vec<ColumnSortKey>,
}

/// Parameters for the GetColumnProfiles method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GetColumnProfilesParams {
	/// Array of requested profiles
	pub profiles: Vec<ColumnProfileRequest>,

	/// Formatting options for returning data values as strings
	pub format_options: FormatOptions,
}

/**
 * Backend RPC request types for the data_explorer comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum DataExplorerBackendRequest {
	/// Request schema
	///
	/// Request full schema for a table-like object
	#[serde(rename = "get_schema")]
	GetSchema(GetSchemaParams),

	/// Search schema by column name
	///
	/// Search schema for column names matching a passed substring
	#[serde(rename = "search_schema")]
	SearchSchema(SearchSchemaParams),

	/// Get a rectangle of data values
	///
	/// Request a rectangular subset of data with values formatted as strings
	#[serde(rename = "get_data_values")]
	GetDataValues(GetDataValuesParams),

	/// Export data selection as a string in different formats
	///
	/// Export data selection as a string in different formats like CSV, TSV,
	/// HTML
	#[serde(rename = "export_data_selection")]
	ExportDataSelection(ExportDataSelectionParams),

	/// Set row filters based on column values
	///
	/// Set or clear row filters on table, replacing any previous filters
	#[serde(rename = "set_row_filters")]
	SetRowFilters(SetRowFiltersParams),

	/// Set or clear sort-by-column(s)
	///
	/// Set or clear the columns(s) to sort by, replacing any previous sort
	/// columns
	#[serde(rename = "set_sort_columns")]
	SetSortColumns(SetSortColumnsParams),

	/// Request a batch of column profiles
	///
	/// Requests a statistical summary or data profile for batch of columns
	#[serde(rename = "get_column_profiles")]
	GetColumnProfiles(GetColumnProfilesParams),

	/// Get the state
	///
	/// Request the current backend state (shape, filters, sort keys,
	/// features)
	#[serde(rename = "get_state")]
	GetState,

}

/**
 * Backend RPC Reply types for the data_explorer comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum DataExplorerBackendReply {
	GetSchemaReply(TableSchema),

	SearchSchemaReply(SearchSchemaResult),

	/// Table values formatted as strings
	GetDataValuesReply(TableData),

	/// Exported result
	ExportDataSelectionReply(ExportedData),

	/// The result of applying filters to a table
	SetRowFiltersReply(FilterResult),

	/// Reply for the set_sort_columns method (no result)
	SetSortColumnsReply(),

	GetColumnProfilesReply(Vec<ColumnProfileResult>),

	/// The current backend state for the data explorer
	GetStateReply(BackendState),

}

/**
 * Frontend RPC request types for the data_explorer comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum DataExplorerFrontendRequest {
}

/**
 * Frontend RPC Reply types for the data_explorer comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum DataExplorerFrontendReply {
}

/**
 * Frontend events for the data_explorer comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum DataExplorerFrontendEvent {
	/// Notify the data explorer to do a state sync after a schema change.
	#[serde(rename = "schema_update")]
	SchemaUpdate,

	/// Triggered when there is any data change detected, clearing cache data
	/// and triggering a refresh/redraw.
	#[serde(rename = "data_update")]
	DataUpdate,

}

