/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from plot.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// A rendered plot
#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct PlotResult {
	/// The plot data, as a base64-encoded string
	pub data: String,

	/// The MIME type of the plot data
	pub mime_type: String
}

/// Parameters for the Render method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct RenderParams {
	/// The requested plot height, in pixels
	pub height: i64,

	/// The requested plot width, in pixels
	pub width: i64,

	/// The pixel ratio of the display device
	pub pixel_ratio: f64,
}

/**
 * Backend RPC request types for the plot comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum PlotBackendRpcRequest {
	/// Render a plot
	///
	/// Requests a plot to be rendered at a given height and width. The plot
	/// data is returned in a base64-encoded string.
	#[serde(rename = "render")]
	Render(RenderParams),

}

/**
 * Backend RPC Reply types for the plot comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum PlotBackendRpcReply {
	/// A rendered plot
	RenderReply(PlotResult),

}

/**
 * Frontend RPC request types for the plot comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum PlotFrontendRpcRequest {
}

/**
 * Frontend RPC Reply types for the plot comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum PlotFrontendRpcReply {
}

/**
 * Frontend events for the plot comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum PlotEvent {
	#[serde(rename = "update")]
	Update,

}

