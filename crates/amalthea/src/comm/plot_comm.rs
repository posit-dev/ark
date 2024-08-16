// @generated

/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from plot.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// The intrinsic size of a plot, if known
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IntrinsicSize {
	/// The width of the plot
	pub width: f64,

	/// The height of the plot
	pub height: f64,

	/// The unit of measurement of the plot's dimensions
	pub unit: PlotUnit,

	/// The source of the intrinsic size e.g. 'Matplotlib'
	pub source: String
}

/// A rendered plot
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PlotResult {
	/// The plot data, as a base64-encoded string
	pub data: String,

	/// The MIME type of the plot data
	pub mime_type: String
}

/// The size of a plot
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PlotSize {
	/// The plot's height, in pixels
	pub height: i64,

	/// The plot's width, in pixels
	pub width: i64
}

/// Possible values for Format in Render
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, strum_macros::Display)]
pub enum RenderFormat {
	#[serde(rename = "png")]
	#[strum(to_string = "png")]
	Png,

	#[serde(rename = "jpeg")]
	#[strum(to_string = "jpeg")]
	Jpeg,

	#[serde(rename = "svg")]
	#[strum(to_string = "svg")]
	Svg,

	#[serde(rename = "pdf")]
	#[strum(to_string = "pdf")]
	Pdf
}

/// Possible values for PlotUnit
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, strum_macros::Display)]
pub enum PlotUnit {
	#[serde(rename = "pixels")]
	#[strum(to_string = "pixels")]
	Pixels,

	#[serde(rename = "inches")]
	#[strum(to_string = "inches")]
	Inches
}

/// Parameters for the Render method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RenderParams {
	/// The requested size of the plot. If not provided, the plot will be
	/// rendered at its intrinsic size.
	pub size: Option<PlotSize>,

	/// The pixel ratio of the display device
	pub pixel_ratio: f64,

	/// The requested plot format
	pub format: RenderFormat,
}

/**
 * Backend RPC request types for the plot comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum PlotBackendRequest {
	/// Get the intrinsic size of a plot, if known.
	///
	/// The intrinsic size of a plot is the size at which a plot would be if
	/// no size constraints were applied by Positron.
	#[serde(rename = "get_intrinsic_size")]
	GetIntrinsicSize,

	/// Render a plot
	///
	/// Requests a plot to be rendered. The plot data is returned in a
	/// base64-encoded string.
	#[serde(rename = "render")]
	Render(RenderParams),

}

/**
 * Backend RPC Reply types for the plot comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum PlotBackendReply {
	/// The intrinsic size of a plot, if known
	GetIntrinsicSizeReply(Option<IntrinsicSize>),

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

	#[serde(rename = "show")]
	Show,

}

