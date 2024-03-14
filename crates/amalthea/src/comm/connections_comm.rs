// @generated

/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024 Posit Software, PBC. All rights reserved.
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

}

