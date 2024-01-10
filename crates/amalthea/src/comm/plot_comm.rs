/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from plot.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// A rendered plot
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PlotResult {
	/// The plot data, as a base64-encoded string
	pub data: String,

	/// The MIME type of the plot data
	pub mime_type: String
}

/// Parameters for the Render method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum PlotBackendRequest {
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
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum PlotBackendReply {
	/// A rendered plot
	RenderReply(PlotResult),

}

/**
 * Frontend RPC request types for the plot comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum PlotFrontendRequest {
}

/**
 * Frontend RPC Reply types for the plot comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum PlotFrontendReply {
}

/**
 * Frontend events for the plot comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum PlotFrontendEvent {
	#[serde(rename = "update")]
	Update,

}

