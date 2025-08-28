// @generated

/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024-2025 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from variables.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// A view containing a list of variables in the session.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VariableList {
	/// A list of variables in the session.
	pub variables: Vec<Variable>,

	/// The total number of variables in the session. This may be greater than
	/// the number of variables in the 'variables' array if the array is
	/// truncated.
	pub length: i64,

	/// The version of the view (incremented with each update)
	pub version: Option<i64>
}

/// An inspected variable.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InspectedVariable {
	/// The children of the inspected variable.
	pub children: Vec<Variable>,

	/// The total number of children. This may be greater than the number of
	/// children in the 'children' array if the array is truncated.
	pub length: i64
}

/// An object formatted for copying to the clipboard.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FormattedVariable {
	/// The formatted content of the variable.
	pub content: String
}

/// Result of the summarize operation
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QueryTableSummaryResult {
	/// The total number of rows in the table.
	pub num_rows: i64,

	/// The total number of columns in the table.
	pub num_columns: i64,

	/// The column schemas in the table.
	pub column_schemas: Vec<String>,

	/// The column profiles in the table.
	pub column_profiles: Vec<String>
}

/// A single variable in the runtime.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Variable {
	/// A key that uniquely identifies the variable within the runtime and can
	/// be used to access the variable in `inspect` requests
	pub access_key: String,

	/// The name of the variable, formatted for display
	pub display_name: String,

	/// A string representation of the variable's value, formatted for display
	/// and possibly truncated
	pub display_value: String,

	/// The variable's type, formatted for display
	pub display_type: String,

	/// Extended information about the variable's type
	pub type_info: String,

	/// The size of the variable's value in bytes
	pub size: i64,

	/// The kind of value the variable represents, such as 'string' or
	/// 'number'
	pub kind: VariableKind,

	/// The number of elements in the variable, if it is a collection
	pub length: i64,

	/// Whether the variable has child variables
	pub has_children: bool,

	/// True if there is a viewer available for this variable (i.e. the
	/// runtime can handle a 'view' request for this variable)
	pub has_viewer: bool,

	/// True if the 'value' field is a truncated representation of the
	/// variable's value
	pub is_truncated: bool,

	/// The time the variable was created or updated, in milliseconds since
	/// the epoch, or 0 if unknown.
	pub updated_time: i64
}

/// Possible values for Format in ClipboardFormat
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, strum_macros::Display, strum_macros::EnumString)]
pub enum ClipboardFormatFormat {
	#[serde(rename = "text/html")]
	#[strum(to_string = "text/html")]
	TextHtml,

	#[serde(rename = "text/plain")]
	#[strum(to_string = "text/plain")]
	TextPlain
}

/// Possible values for Kind in Variable
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, strum_macros::Display, strum_macros::EnumString)]
pub enum VariableKind {
	#[serde(rename = "boolean")]
	#[strum(to_string = "boolean")]
	Boolean,

	#[serde(rename = "bytes")]
	#[strum(to_string = "bytes")]
	Bytes,

	#[serde(rename = "class")]
	#[strum(to_string = "class")]
	Class,

	#[serde(rename = "collection")]
	#[strum(to_string = "collection")]
	Collection,

	#[serde(rename = "empty")]
	#[strum(to_string = "empty")]
	Empty,

	#[serde(rename = "function")]
	#[strum(to_string = "function")]
	Function,

	#[serde(rename = "map")]
	#[strum(to_string = "map")]
	Map,

	#[serde(rename = "number")]
	#[strum(to_string = "number")]
	Number,

	#[serde(rename = "other")]
	#[strum(to_string = "other")]
	Other,

	#[serde(rename = "string")]
	#[strum(to_string = "string")]
	String,

	#[serde(rename = "table")]
	#[strum(to_string = "table")]
	Table,

	#[serde(rename = "lazy")]
	#[strum(to_string = "lazy")]
	Lazy,

	#[serde(rename = "connection")]
	#[strum(to_string = "connection")]
	Connection
}

/// Parameters for the Clear method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ClearParams {
	/// Whether to clear hidden objects in addition to normal variables
	pub include_hidden_objects: bool,
}

/// Parameters for the Delete method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeleteParams {
	/// The names of the variables to delete.
	pub names: Vec<String>,
}

/// Parameters for the Inspect method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct InspectParams {
	/// The path to the variable to inspect, as an array of access keys.
	pub path: Vec<String>,
}

/// Parameters for the ClipboardFormat method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ClipboardFormatParams {
	/// The path to the variable to format, as an array of access keys.
	pub path: Vec<String>,

	/// The requested format for the variable, as a MIME type
	pub format: ClipboardFormatFormat,
}

/// Parameters for the View method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ViewParams {
	/// The path to the variable to view, as an array of access keys.
	pub path: Vec<String>,
}

/// Parameters for the QueryTableSummary method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct QueryTableSummaryParams {
	/// The path to the table to summarize, as an array of access keys.
	pub path: Vec<String>,

	/// A list of query types.
	pub query_types: Vec<String>,
}

/// Parameters for the Update method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UpdateParams {
	/// An array of variables that have been newly assigned.
	pub assigned: Vec<Variable>,

	/// An array of variables that were not evaluated for value updates.
	pub unevaluated: Vec<Variable>,

	/// An array of variable names that have been removed.
	pub removed: Vec<String>,

	/// The version of the view (incremented with each update), or 0 if the
	/// backend doesn't track versions.
	pub version: i64,
}

/// Parameters for the Refresh method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RefreshParams {
	/// An array listing all the variables in the current session.
	pub variables: Vec<Variable>,

	/// The number of variables in the current session.
	pub length: i64,

	/// The version of the view (incremented with each update), or 0 if the
	/// backend doesn't track versions.
	pub version: i64,
}

/**
 * Backend RPC request types for the variables comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum VariablesBackendRequest {
	/// List all variables
	///
	/// Returns a list of all the variables in the current session.
	#[serde(rename = "list")]
	List,

	/// Clear all variables
	///
	/// Clears (deletes) all variables in the current session.
	#[serde(rename = "clear")]
	Clear(ClearParams),

	/// Deletes a set of named variables
	///
	/// Deletes the named variables from the current session.
	#[serde(rename = "delete")]
	Delete(DeleteParams),

	/// Inspect a variable
	///
	/// Returns the children of a variable, as an array of variables.
	#[serde(rename = "inspect")]
	Inspect(InspectParams),

	/// Format for clipboard
	///
	/// Requests a formatted representation of a variable for copying to the
	/// clipboard.
	#[serde(rename = "clipboard_format")]
	ClipboardFormat(ClipboardFormatParams),

	/// Request a viewer for a variable
	///
	/// Request that the runtime open a data viewer to display the data in a
	/// variable.
	#[serde(rename = "view")]
	View(ViewParams),

	/// Query table summary
	///
	/// Request a data summary for a table variable.
	#[serde(rename = "query_table_summary")]
	QueryTableSummary(QueryTableSummaryParams),

}

/**
 * Backend RPC Reply types for the variables comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum VariablesBackendReply {
	/// A view containing a list of variables in the session.
	ListReply(VariableList),

	/// Reply for the clear method (no result)
	ClearReply(),

	/// The names of the variables that were successfully deleted.
	DeleteReply(Vec<String>),

	/// An inspected variable.
	InspectReply(InspectedVariable),

	/// An object formatted for copying to the clipboard.
	ClipboardFormatReply(FormattedVariable),

	/// The ID of the viewer that was opened.
	ViewReply(Option<String>),

	/// Result of the summarize operation
	QueryTableSummaryReply(QueryTableSummaryResult),

}

/**
 * Frontend RPC request types for the variables comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum VariablesFrontendRequest {
}

/**
 * Frontend RPC Reply types for the variables comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum VariablesFrontendReply {
}

/**
 * Frontend events for the variables comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum VariablesFrontendEvent {
	/// Updates the variables in the current session.
	#[serde(rename = "update")]
	Update(UpdateParams),

	/// Replace all variables in the current session with the variables from
	/// the backend.
	#[serde(rename = "refresh")]
	Refresh(RefreshParams),

}

