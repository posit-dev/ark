// @generated

/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024-2025 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from connections.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// ObjectSchema in Schemas
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ObjectSchema {
	/// Name of the underlying object
	pub name: String,

	/// The object type (table, catalog, schema)
	pub kind: String
}

/// FieldSchema in Schemas
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FieldSchema {
	/// Name of the field
	pub name: String,

	/// The field data type
	pub dtype: String
}

/// MetadataSchema in Schemas
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct MetadataSchema {
	/// Connection name
	pub name: String,

	/// Language ID for the connections. Essentially just R or python
	pub language_id: String,

	/// Connection host
	pub host: Option<String>,

	/// Connection type
	#[serde(rename = "type")]
	pub metadata_schema_type: Option<String>,

	/// Code used to re-create the connection
	pub code: Option<String>
}

/// Parameters for the ListObjects method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ListObjectsParams {
	/// The path to object that we want to list children.
	pub path: Vec<ObjectSchema>,
}

/// Parameters for the ListFields method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ListFieldsParams {
	/// The path to object that we want to list fields.
	pub path: Vec<ObjectSchema>,
}

/// Parameters for the ContainsData method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ContainsDataParams {
	/// The path to object that we want to check if it contains data.
	pub path: Vec<ObjectSchema>,
}

/// Parameters for the GetIcon method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GetIconParams {
	/// The path to object that we want to get the icon.
	pub path: Vec<ObjectSchema>,
}

/// Parameters for the PreviewObject method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PreviewObjectParams {
	/// The path to object that we want to preview.
	pub path: Vec<ObjectSchema>,
}

/// Parameters for the GetMetadata method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct GetMetadataParams {
	/// The comm_id of the client we want to retrieve metdata for.
	pub comm_id: String,
}

/**
 * Backend RPC request types for the connections comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum ConnectionsBackendRequest {
	/// List objects within a data source
	///
	/// List objects within a data source, such as schemas, catalogs, tables
	/// and views.
	#[serde(rename = "list_objects")]
	ListObjects(ListObjectsParams),

	/// List fields of an object
	///
	/// List fields of an object, such as columns of a table or view.
	#[serde(rename = "list_fields")]
	ListFields(ListFieldsParams),

	/// Check if an object contains data
	///
	/// Check if an object contains data, such as a table or view.
	#[serde(rename = "contains_data")]
	ContainsData(ContainsDataParams),

	/// Get icon of an object
	///
	/// Get icon of an object, such as a table or view.
	#[serde(rename = "get_icon")]
	GetIcon(GetIconParams),

	/// Preview object data
	///
	/// Preview object data, such as a table or view.
	#[serde(rename = "preview_object")]
	PreviewObject(PreviewObjectParams),

	/// Gets metadata from the connections
	///
	/// A connection has tied metadata such as an icon, the host, etc.
	#[serde(rename = "get_metadata")]
	GetMetadata(GetMetadataParams),

}

/**
 * Backend RPC Reply types for the connections comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum ConnectionsBackendReply {
	/// Array of objects names and their kinds.
	ListObjectsReply(Vec<ObjectSchema>),

	/// Array of field names and data types.
	ListFieldsReply(Vec<FieldSchema>),

	/// Boolean indicating if the object contains data.
	ContainsDataReply(bool),

	/// The icon of the object.
	GetIconReply(String),

	PreviewObjectReply(),

	GetMetadataReply(MetadataSchema),

}

/**
 * Frontend RPC request types for the connections comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum ConnectionsFrontendRequest {
}

/**
 * Frontend RPC Reply types for the connections comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum ConnectionsFrontendReply {
}

/**
 * Frontend events for the connections comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum ConnectionsFrontendEvent {
	#[serde(rename = "focus")]
	Focus,

	#[serde(rename = "update")]
	Update,

}

