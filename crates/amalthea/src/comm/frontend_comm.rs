/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from frontend.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// Items in Params
pub type Param = serde_json::Value;

/// The method result
pub type CallMethodResult = serde_json::Value;

/// Parameters for the CallMethod method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct CallMethodParams {
	/// The method to call inside the interpreter
	pub method: String,

	/// The parameters for `method`
	pub params: Vec<Param>,
}

/// Parameters for the Busy method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct BusyParams {
	/// Whether the backend is busy
	pub busy: bool,
}

/// Parameters for the OpenEditor method.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct OpenEditorParams {
	/// The path of the file to open
	pub file: String,

	/// The line number to jump to
	pub line: i64,

	/// The column number to jump to
	pub column: i64,
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
	/// This represents the busy state of the underlying computation engine,
	/// not the busy state of the kernel. The kernel is busy when it is
	/// processing a request, but the runtime is busy only when a computation
	/// is running.
	#[serde(rename = "busy")]
	Busy(BusyParams),

	/// Use this to clear the console.
	#[serde(rename = "clear_console")]
	ClearConsole,

	/// This event is used to open an editor with a given file and selection.
	#[serde(rename = "open_editor")]
	OpenEditor(OpenEditorParams),

	/// Use this for messages that require immediate attention from the user
	#[serde(rename = "show_message")]
	ShowMessage(ShowMessageParams),

	/// Languages like R allow users to change the way their prompts look.
	/// This event signals a change in the prompt configuration.
	#[serde(rename = "prompt_state")]
	PromptState(PromptStateParams),

	/// This event signals a change in the working direcotry of the
	/// interpreter
	#[serde(rename = "working_directory")]
	WorkingDirectory(WorkingDirectoryParams),

}

