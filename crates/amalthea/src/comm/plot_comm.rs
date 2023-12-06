/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from plot.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct PlotResult {
	/// The plot data, as a base64-encoded string
	pub data: i64,

	/// The MIME type of the plot data
	pub mime_type: String,

}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct RenderParams {
	/// The requested plot height, in pixels
	pub height: i64,

	/// The requested plot width, in pixels
	pub width: i64,

	/// The pixel ratio of the display device
	pub pixel_ratio: i64,

}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct UpdateParams {
}

/**
 * RPC request types for the plot comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum PlotRpcRequest {
	/// Render a plot: Requests a plot to be rendered at a given height and
	/// width. The plot data is returned in a base64-encoded string.
	#[serde(rename = "render")]
	Render(RenderParams),
}

/**
 * RPC Reply types for the plot comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum PlotRpcReply {
	/// A rendered plot
	RenderReply(PlotResult),
}

/**
 * Front-end events for the plot comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum PlotEvent {
	#[serde(rename = "update")]
	Update,
}

