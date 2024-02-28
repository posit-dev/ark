/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from data_explorer.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// The schema for a table-like object
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TableSchema {
	/// Schema for each column in the table
	pub columns: Vec<ColumnSchema>
}

/// Table values formatted as strings
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TableData {
	/// The columns of data
	pub columns: Vec<Vec<String>>,

	/// Zero or more arrays of row labels
	pub row_labels: Option<Vec<Vec<String>>>
}

/// The result of applying filters to a table
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FilterResult {
	/// Number of rows in table after applying filters
	pub selected_num_rows: i64
}

/// Result of computing column profile
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProfileResult {
	/// Number of null values in column
	pub null_count: i64,

	/// Minimum value as string computed as part of histogram
	pub min_value: Option<String>,

	/// Maximum value as string computed as part of histogram
	pub max_value: Option<String>,

	/// Average value as string computed as part of histogram
	pub mean_value: Option<String>,

	/// Absolute count of values in each histogram bin
	pub histogram_bin_sizes: Option<Vec<i64>>,

	/// Absolute floating-point width of a histogram bin
	pub histogram_bin_width: Option<f64>,

	/// Quantile values computed from histogram bins
	pub histogram_quantiles: Option<Vec<ColumnQuantileValue>>,

	/// Counts of distinct values in column
	pub freqtable_counts: Option<Vec<FreqtableCounts>>,

	/// Number of other values not accounted for in counts
	pub freqtable_other_count: Option<i64>
}

/// Items in FreqtableCounts
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FreqtableCounts {
	/// Stringified value
	pub value: String,

	/// Number of occurrences of value
	pub count: i64
}

/// The current backend table state
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TableState {
	/// Provides number of rows and columns in table
	pub table_shape: TableShape,

	/// The set of currently applied filters
	pub filters: Vec<ColumnFilter>,

	/// The set of currently applied sorts
	pub sort_keys: Vec<ColumnSortKey>
}

/// Provides number of rows and columns in table
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TableShape {
	/// Numbers of rows in the unfiltered dataset
	pub num_rows: i64,

	/// Number of columns in the unfiltered dataset
	pub num_columns: i64
}

/// Schema for a column in a table
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnSchema {
	/// Name of column as UTF-8 string
	pub column_name: String,

	/// Exact name of data type used by underlying table
	pub type_name: String,

	/// Canonical Positron display name of data type
	pub type_display: ColumnSchemaTypeDisplay,

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

/// Specifies a table row filter based on a column's values
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ColumnFilter {
	/// Unique identifier for this filter
	pub filter_id: String,

	/// Type of filter to apply
	pub filter_type: ColumnFilterFilterType,

	/// Column index to apply filter to
	pub column_index: i64,

	/// String representation of a binary comparison
	pub compare_op: Option<ColumnFilterCompareOp>,

	/// A stringified column value for a comparison filter
	pub compare_value: Option<String>,

	/// Array of column values for a set membership filter
	pub set_member_values: Option<Vec<String>>,

	/// Filter by including only values passed (true) or excluding (false)
	pub set_member_inclusive: Option<bool>,

	/// Type of search to perform
	pub search_type: Option<ColumnFilterSearchType>,

	/// String value/regex to search for in stringified data
	pub search_term: Option<String>,

	/// If true, do a case-sensitive search, otherwise case-insensitive
	pub search_case_sensitive: Option<bool>
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

/// Possible values for ProfileType in GetColumnProfile
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum GetColumnProfileProfileType {
	#[serde(rename = "freqtable")]
	Freqtable,

	#[serde(rename = "histogram")]
	Histogram
}

/// Possible values for TypeDisplay in ColumnSchema
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ColumnSchemaTypeDisplay {
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

/// Possible values for FilterType in ColumnFilter
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ColumnFilterFilterType {
	#[serde(rename = "isnull")]
	Isnull,

	#[serde(rename = "notnull")]
	Notnull,

	#[serde(rename = "compare")]
	Compare,

	#[serde(rename = "set_membership")]
	SetMembership,

	#[serde(rename = "search")]
	Search
}

/// Possible values for CompareOp in ColumnFilter
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ColumnFilterCompareOp {
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

/// Possible values for SearchType in ColumnFilter
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ColumnFilterSearchType {
	#[serde(rename = "contains")]
	Contains,

	#[serde(rename = "startswith")]
	Startswith,

	#[serde(rename = "endswith")]
	Endswith,

	#[serde(rename = "regex")]
	Regex
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
}

/// Parameters for the SetColumnFilters method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SetColumnFiltersParams {
	/// Zero or more filters to apply
	pub filters: Vec<ColumnFilter>,
}

/// Parameters for the SetSortColumns method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SetSortColumnsParams {
	/// Pass zero or more keys to sort by. Clears any existing keys
	pub sort_keys: Vec<ColumnSortKey>,
}

/// Parameters for the GetColumnProfile method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GetColumnProfileParams {
	/// The type of analytical column profile
	pub profile_type: GetColumnProfileProfileType,

	/// Column index to compute profile for
	pub column_index: i64,
}

/// Parameters for the SchemaUpdate method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SchemaUpdateParams {
	/// If true, the UI should discard the filter/sort state.
	pub discard_state: bool,
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

	/// Get a rectangle of data values
	///
	/// Request a rectangular subset of data with values formatted as strings
	#[serde(rename = "get_data_values")]
	GetDataValues(GetDataValuesParams),

	/// Set column filters
	///
	/// Set or clear column filters on table, replacing any previous filters
	#[serde(rename = "set_column_filters")]
	SetColumnFilters(SetColumnFiltersParams),

	/// Set or clear sort-by-column(s)
	///
	/// Set or clear the columns(s) to sort by, replacing any previous sort
	/// columns
	#[serde(rename = "set_sort_columns")]
	SetSortColumns(SetSortColumnsParams),

	/// Get a column profile
	///
	/// Requests a statistical summary or data profile for a column
	#[serde(rename = "get_column_profile")]
	GetColumnProfile(GetColumnProfileParams),

	/// Get the state
	///
	/// Request the current table state (applied filters and sort columns)
	#[serde(rename = "get_state")]
	GetState,

}

/**
 * Backend RPC Reply types for the data_explorer comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum DataExplorerBackendReply {
	/// The schema for a table-like object
	GetSchemaReply(TableSchema),

	/// Table values formatted as strings
	GetDataValuesReply(TableData),

	/// The result of applying filters to a table
	SetColumnFiltersReply(FilterResult),

	/// Reply for the set_sort_columns method (no result)
	SetSortColumnsReply(),

	/// Result of computing column profile
	GetColumnProfileReply(ProfileResult),

	/// The current backend table state
	GetStateReply(TableState),

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
	/// Fully reset and redraw the data explorer after a schema change.
	#[serde(rename = "schema_update")]
	SchemaUpdate(SchemaUpdateParams),

	/// Triggered when there is any data change detected, clearing cache data
	/// and triggering a refresh/redraw.
	#[serde(rename = "data_update")]
	DataUpdate,

}

