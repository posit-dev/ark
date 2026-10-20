// @generated

/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024-2026 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from object_explorer.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// The state of an object explorer
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ObjectExplorerState {
	/// Title for the editor tab, usually the variable name or expression that
	/// was viewed
	pub title: String,

	/// False when the explored object no longer exists; the frontend shows a
	/// disconnected state
	pub connected: bool,

	/// Optional message explaining why the explorer is disconnected
	pub error_message: Option<String>
}

/// A page of child nodes
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ChildrenResult {
	/// The requested page of children, in the object's natural order
	pub children: Vec<ObjectNode>,

	/// Total number of children the parent has (may exceed the number
	/// returned)
	pub total: i64
}

/// Search results
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
	/// Matches and their ancestors in pre-order
	pub rows: Vec<SearchRow>,

	/// Number of matching nodes in 'rows'
	pub total_matches: i64,

	/// True if the search stopped early because max_results or the node
	/// budget was reached
	pub truncated: bool
}

/// A value formatted as plain text
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FormattedValue {
	/// The formatted value
	pub content: String,

	/// Whether the content was cut at max_length
	pub is_truncated: bool
}

/// One node in the explored object
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ObjectNode {
	/// The path segment that selects this node within its parent; same
	/// semantics as the variables comm
	pub access_key: String,

	/// The node's name (key, index, slot, or attribute), formatted for
	/// display
	pub display_name: String,

	/// The node's type, formatted for display using the same formatter as the
	/// variables comm
	pub display_type: String,

	/// The node's value, formatted for display and possibly truncated, using
	/// the same formatter as the variables comm
	pub display_value: String,

	/// The kind of value, using the same vocabulary as the variables comm
	pub kind: ObjectNodeKind,

	/// The number of children or elements, if known; 0 otherwise
	pub length: i64,

	/// Whether get_children would return anything for this node
	pub has_children: bool,

	/// True if display_value is a truncated representation
	pub is_truncated: bool,

	/// True if this node is the same object as one of its ancestors; such
	/// nodes never report children
	pub is_cycle: bool,

	/// A language expression that evaluates to this node's value. Absent when
	/// no expression exists.
	pub accessor: Option<String>
}

/// A row in a search result: a match or an ancestor of a match
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SearchRow {
	/// Access keys from the root to this node
	pub path: Vec<String>,

	/// The node at this path
	pub node: ObjectNode,

	/// Which field matched; 'ancestor' for ancestors included only for
	/// context
	pub match_kind: SearchRowMatchKind
}

/// Possible values for Kind in ObjectNode
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, strum_macros::Display, strum_macros::EnumString)]
pub enum ObjectNodeKind {
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

/// Possible values for MatchKind in SearchRow
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, strum_macros::Display, strum_macros::EnumString)]
pub enum SearchRowMatchKind {
	#[serde(rename = "ancestor")]
	#[strum(to_string = "ancestor")]
	Ancestor,

	#[serde(rename = "name")]
	#[strum(to_string = "name")]
	Name,

	#[serde(rename = "value")]
	#[strum(to_string = "value")]
	Value,

	#[serde(rename = "name_and_value")]
	#[strum(to_string = "name_and_value")]
	NameAndValue
}

/// Parameters for the GetChildren method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GetChildrenParams {
	/// Access keys from the root to the parent node; [] is the root
	pub path: Vec<String>,

	/// Zero-based index of the first child to return
	pub start: i64,

	/// Maximum number of children to return
	pub limit: i64,
}

/// Parameters for the Search method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SearchParams {
	/// The text to search for
	pub query: String,

	/// Maximum depth below the root to descend (root children are depth 1)
	pub max_depth: i64,

	/// Maximum number of matching nodes to return
	pub max_results: i64,
}

/// Parameters for the FormatValue method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FormatValueParams {
	/// Access keys from the root to the node
	pub path: Vec<String>,

	/// The most characters to return. The whole value is returned when
	/// omitted.
	pub max_length: Option<i64>,
}

/// Parameters for the ViewTable method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ViewTableParams {
	/// Access keys from the root to the node
	pub path: Vec<String>,

	/// Title for the data explorer, usually the node's display name
	pub title: String,
}

/**
 * Backend RPC request types for the object_explorer comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum ObjectExplorerBackendRequest {
	/// Get the explorer state
	///
	/// Returns the title and connection state of the explored object. Used on
	/// first open and when reconnecting to an existing comm.
	#[serde(rename = "get_state")]
	GetState,

	/// Get the root node
	///
	/// Returns the node describing the explored object itself (path []).
	#[serde(rename = "get_root")]
	GetRoot,

	/// Get one page of a node's children
	///
	/// Returns up to 'limit' children of the node at 'path', starting at
	/// child index 'start'. Never descends more than one level.
	#[serde(rename = "get_children")]
	GetChildren(GetChildrenParams),

	/// Search names and values
	///
	/// Depth-first, case-insensitive substring search over node display
	/// names, and over the display values of leaves (nodes that are neither
	/// containers nor cycles), bounded by max_depth and an internal node
	/// budget. Returns matches and every ancestor of a match, in pre-order,
	/// so the frontend can render the results as a tree.
	#[serde(rename = "search")]
	Search(SearchParams),

	/// Format a node's value as plain text
	///
	/// Returns the plain-text representation of the node at 'path', for the
	/// clipboard or for reading in full. Strings are returned as their
	/// contents, without quotes or escapes.
	#[serde(rename = "format_value")]
	FormatValue(FormatValueParams),

	/// Open a node in the Data Explorer
	///
	/// Asks the backend to open a data explorer comm on the node at 'path',
	/// which must be a table. Returns the new comm id.
	#[serde(rename = "view_table")]
	ViewTable(ViewTableParams),

	/// Open a full object explorer for an inline explorer
	///
	/// Asks the backend to open a new, non-inline object explorer comm on the
	/// same object. Returns the new comm id.
	#[serde(rename = "open_object_explorer")]
	OpenObjectExplorer,

}

/**
 * Backend RPC Reply types for the object_explorer comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
#[allow(clippy::large_enum_variant)]
pub enum ObjectExplorerBackendReply {
	/// The state of an object explorer
	GetStateReply(ObjectExplorerState),

	GetRootReply(ObjectNode),

	/// A page of child nodes
	GetChildrenReply(ChildrenResult),

	/// Search results
	SearchReply(SearchResult),

	/// A value formatted as plain text
	FormatValueReply(FormattedValue),

	/// The comm id of the newly opened data explorer
	ViewTableReply(String),

	/// The comm id of the newly opened object explorer
	OpenObjectExplorerReply(String),

}

/**
 * Frontend RPC request types for the object_explorer comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum ObjectExplorerFrontendRequest {
}

/**
 * Frontend RPC Reply types for the object_explorer comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
#[allow(clippy::large_enum_variant)]
pub enum ObjectExplorerFrontendReply {
}

/**
 * Frontend events for the object_explorer comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum ObjectExplorerFrontendEvent {
	/// Sent after the backend detects that the explored object was reassigned
	/// or otherwise changed. The frontend re-fetches every loaded level,
	/// preserving expansion.
	#[serde(rename = "update")]
	Update,

}

