/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from frontend.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// Parameters for the Busy method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct BusyParams {
	/// Whether the backend is busy
	pub busy: bool,
}

/// Parameters for the ShowMessage method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowMessageParams {
	/// The message to show to the user.
	pub message: String,
}

/// Parameters for the PromptState method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct PromptStateParams {
	/// Prompt for primary input.
	pub input_prompt: String,

	/// Prompt for incomplete input.
	pub continuation_prompt: String,
}

/// Parameters for the WorkingDirectory method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkingDirectoryParams {
	/// The new working directory
	pub directory: String,
}

/**
 * RPC request types for the frontend comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum FrontendRpcRequest {
}

/**
 * RPC Reply types for the frontend comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum FrontendRpcReply {
}

/**
 * Front-end events for the frontend comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum FrontendEvent {
	#[serde(rename = "busy")]
	Busy(BusyParams),
	#[serde(rename = "show_message")]
	ShowMessage(ShowMessageParams),
	#[serde(rename = "prompt_state")]
	PromptState(PromptStateParams),
	#[serde(rename = "working_directory")]
	WorkingDirectory(WorkingDirectoryParams),
}

