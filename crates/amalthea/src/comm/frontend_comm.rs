/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from frontend.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// Items in Params
pub type Params = serde_json::Value;

/// The method result
pub type CallMethodResult = serde_json::Value;

/// Parameters for the CallMethod method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CallMethodParams {
	/// The method to call inside the interpreter
	pub method: String,

	/// The parameters for `method`
	pub params: Vec<Params>,
}

/// Parameters for the Busy method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BusyParams {
	/// Whether the backend is busy
	pub busy: bool,
}

/// Parameters for the ShowMessage method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ShowMessageParams {
	/// The message to show to the user.
	pub message: String,
}

/// Parameters for the PromptState method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PromptStateParams {
	/// Prompt for primary input.
	pub input_prompt: String,

	/// Prompt for incomplete input.
	pub continuation_prompt: String,
}

/// Parameters for the WorkingDirectory method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
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
	/// Run a method in the interpreter and return the result to the frontend
	///
	/// Unlike other RPC methods, `call_method` calls into methods implemented
	/// in the interpreter and returns the result back to the frontend using
	/// an implementation-defined serialization scheme.
	#[serde(rename = "call_method")]
	CallMethod(CallMethodParams),

}

/**
 * RPC Reply types for the frontend comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum FrontendRpcReply {
	/// The method result
	CallMethodReply(CallMethodResult),

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

