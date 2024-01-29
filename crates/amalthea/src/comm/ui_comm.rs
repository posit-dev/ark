/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from ui.json; do not edit.
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
	/// Document metadata
	pub document: UiTextDocument,

	/// Document contents
	pub contents: Vec<String>,

	/// The primary selection, i.e. selections[0]
	pub selection: UiSelection,

	/// The selections in this text editor.
	pub selections: Vec<UiSelection>
}

/// Document metadata
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UiTextDocument {
	/// URI of the resource viewed in the editor
	pub path: String,

	/// End of line sequence
	pub eol: String,

	/// Whether the document has been closed
	pub isClosed: bool,

	/// Whether the document has been modified
	pub isDirty: bool,

	/// Whether the document is untitled
	pub isUntitled: bool,

	/// Language identifier
	pub languageId: String,

	/// Number of lines in the document
	pub lineCount: i64,

	/// Version number of the document
	pub version: i64
}

/// A line and character position, such as the position of the cursor.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UiPosition {
	/// The zero-based character value, as a Unicode code point offset.
	pub character: i64,

	/// The zero-based line value.
	pub line: i64
}

/// Selection metadata
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UiSelection {
	/// Position of the cursor.
	pub active: UiPosition,

	/// Start position of the selection
	pub start: UiPosition,

	/// End position of the selection
	pub end: UiPosition,

	/// Text of the selection
	pub text: String
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
 * Backend RPC request types for the ui comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum UiBackendRequest {
	/// Run a method in the interpreter and return the result to the frontend
	///
	/// Unlike other RPC methods, `call_method` calls into methods implemented
	/// in the interpreter and returns the result back to the frontend using
	/// an implementation-defined serialization scheme.
	#[serde(rename = "call_method")]
	CallMethod(CallMethodParams),

}

/**
 * Backend RPC Reply types for the ui comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum UiBackendReply {
	/// The method result
	CallMethodReply(CallMethodResult),

}

/**
 * Frontend RPC request types for the ui comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum UiFrontendRequest {
	/// Sleep for n seconds
	///
	/// Useful for testing in the backend a long running frontend method
	#[serde(rename = "debug_sleep")]
	DebugSleep(DebugSleepParams),

	/// Context metadata for the last editor
	///
	/// Returns metadata such as file path for the last editor selected by the
	/// user. The result may be undefined if there are no active editors.
	#[serde(rename = "last_active_editor_context")]
	LastActiveEditorContext,

}

/**
 * Frontend RPC Reply types for the ui comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum UiFrontendReply {
	/// Reply for the debug_sleep method (no result)
	DebugSleepReply(),

	/// Editor metadata
	LastActiveEditorContextReply(Option<EditorContextResult>),

}

/**
 * Frontend events for the ui comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum UiFrontendEvent {
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

/**
* Conversion of JSON values to frontend RPC Reply types
*/
pub fn ui_frontend_reply_from_value(
	reply: serde_json::Value,
	request: &UiFrontendRequest,
) -> anyhow::Result<UiFrontendReply> {
	match request {
		UiFrontendRequest::DebugSleep(_) => Ok(UiFrontendReply::DebugSleepReply()),
		UiFrontendRequest::LastActiveEditorContext => Ok(UiFrontendReply::LastActiveEditorContextReply(serde_json::from_value(reply)?)),
	}
}

