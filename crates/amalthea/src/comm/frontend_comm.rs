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

/// Editor metadata
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EditorContextResult {
	/// URI of the resource viewed in the editor
	pub path: String
}

/// Parameters for the CallMethod method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CallMethodParams {
	/// The method to call inside the interpreter
	pub method: String,

	/// The parameters for `method`
	pub params: Vec<Param>,
}

/// Parameters for the Busy method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BusyParams {
	/// Whether the backend is busy
	pub busy: bool,
}

/// Parameters for the OpenEditor method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OpenEditorParams {
	/// The path of the file to open
	pub file: String,

	/// The line number to jump to
	pub line: i64,

	/// The column number to jump to
	pub column: i64,
}

/// Parameters for the ShowMessage method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowMessageParams {
	/// The message to show to the user.
	pub message: String,
}

/// Parameters for the PromptState method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PromptStateParams {
	/// Prompt for primary input.
	pub input_prompt: String,

	/// Prompt for incomplete input.
	pub continuation_prompt: String,
}

/// Parameters for the WorkingDirectory method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkingDirectoryParams {
	/// The new working directory
	pub directory: String,
}

/// Parameters for the DebugSleep method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DebugSleepParams {
	/// Duration in milliseconds
	pub ms: f64,
}

/**
 * Backend RPC request types for the frontend comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum FrontendBackendRpcRequest {
	/// Run a method in the interpreter and return the result to the frontend
	///
	/// Unlike other RPC methods, `call_method` calls into methods implemented
	/// in the interpreter and returns the result back to the frontend using
	/// an implementation-defined serialization scheme.
	#[serde(rename = "call_method")]
	CallMethod(CallMethodParams),

}

/**
 * Backend RPC Reply types for the frontend comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum FrontendBackendRpcReply {
	/// The method result
	CallMethodReply(CallMethodResult),

}

/**
 * Frontend RPC request types for the frontend comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum FrontendFrontendRpcRequest {
	/// Context metadata for the last editor
	///
	/// Returns metadata such as file path for the last editor selected by the
	/// user. The result may be undefined if there are no active editors.
	#[serde(rename = "last_active_editor_context")]
	LastActiveEditorContext,

	/// Sleep for n seconds
	///
	/// Useful for testing in the backend a long running frontend method
	#[serde(rename = "debug_sleep")]
	DebugSleep(DebugSleepParams),

}

/**
 * Frontend RPC Reply types for the frontend comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum FrontendFrontendRpcReply {
	/// Editor metadata
	LastActiveEditorContextReply(Option<EditorContextResult>),

}

/**
 * Frontend events for the frontend comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum FrontendEvent {
	#[serde(rename = "busy")]
	Busy(BusyParams),

	#[serde(rename = "clear_console")]
	ClearConsole,

	#[serde(rename = "open_editor")]
	OpenEditor(OpenEditorParams),

	#[serde(rename = "show_message")]
	ShowMessage(ShowMessageParams),

	#[serde(rename = "prompt_state")]
	PromptState(PromptStateParams),

	#[serde(rename = "working_directory")]
	WorkingDirectory(WorkingDirectoryParams),

}

