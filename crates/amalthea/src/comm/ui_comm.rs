// @generated

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
pub struct EditorContext {
	/// Document metadata
	pub document: TextDocument,

	/// Document contents
	pub contents: Vec<String>,

	/// The primary selection, i.e. selections[0]
	pub selection: Selection,

	/// The selections in this text editor.
	pub selections: Vec<Selection>
}

/// Document metadata
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TextDocument {
	/// URI of the resource viewed in the editor
	pub path: String,

	/// End of line sequence
	pub eol: String,

	/// Whether the document has been closed
	pub is_closed: bool,

	/// Whether the document has been modified
	pub is_dirty: bool,

	/// Whether the document is untitled
	pub is_untitled: bool,

	/// Language identifier
	pub language_id: String,

	/// Number of lines in the document
	pub line_count: i64,

	/// Version number of the document
	pub version: i64
}

/// A line and character position, such as the position of the cursor.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Position {
	/// The zero-based character value, as a Unicode code point offset.
	pub character: i64,

	/// The zero-based line value.
	pub line: i64
}

/// Selection metadata
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Selection {
	/// Position of the cursor.
	pub active: Position,

	/// Start position of the selection
	pub start: Position,

	/// End position of the selection
	pub end: Position,

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

/// Parameters for the ShowQuestion method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowQuestionParams {
	/// The title of the dialog
	pub title: String,

	/// The message to display in the dialog
	pub message: String,

	/// The title of the OK button
	pub ok_button_title: String,

	/// The title of the Cancel button
	pub cancel_button_title: String,
}

/// Parameters for the ShowDialog method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowDialogParams {
	/// The title of the dialog
	pub title: String,

	/// The message to display in the dialog
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

/// Parameters for the ExecuteCommand method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExecuteCommandParams {
	/// The command to execute
	pub command: String,
}

/// Parameters for the OpenWorkspace method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OpenWorkspaceParams {
	/// The path for the workspace to be opened
	pub path: String,

	/// Should the workspace be opened in a new window?
	pub new_window: bool,
}

/// Parameters for the ShowUrl method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowUrlParams {
	/// The URL to display
	pub url: String,
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
	/// Show a question
	///
	/// Use this for a modal dialog that the user can accept or cancel
	#[serde(rename = "show_question")]
	ShowQuestion(ShowQuestionParams),

	/// Show a dialog
	///
	/// Use this for a modal dialog that the user can only accept
	#[serde(rename = "show_dialog")]
	ShowDialog(ShowDialogParams),

	/// Sleep for n seconds
	///
	/// Useful for testing in the backend a long running frontend method
	#[serde(rename = "debug_sleep")]
	DebugSleep(DebugSleepParams),

	/// Path to the workspace folder
	///
	/// Returns the path to the workspace folder, or first folder if there are
	/// multiple.
	#[serde(rename = "workspace_folder")]
	WorkspaceFolder,

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
	/// Whether the user accepted or rejected the dialog.
	ShowQuestionReply(bool),

	/// Reply for the show_dialog method (no result)
	ShowDialogReply(),

	/// Reply for the debug_sleep method (no result)
	DebugSleepReply(),

	/// The path to the workspace folder
	WorkspaceFolderReply(Option<String>),

	/// Editor metadata
	LastActiveEditorContextReply(Option<EditorContext>),

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

	/// Use this to execute a Positron command from the backend (like from a
	/// runtime)
	#[serde(rename = "execute_command")]
	ExecuteCommand(ExecuteCommandParams),

	/// Use this to open a workspace in Positron
	#[serde(rename = "open_workspace")]
	OpenWorkspace(OpenWorkspaceParams),

	/// Causes the URL to be displayed inside the Viewer pane, and makes the
	/// Viewer pane visible.
	#[serde(rename = "show_url")]
	ShowUrl(ShowUrlParams),

}

/**
* Conversion of JSON values to frontend RPC Reply types
*/
pub fn ui_frontend_reply_from_value(
	reply: serde_json::Value,
	request: &UiFrontendRequest,
) -> anyhow::Result<UiFrontendReply> {
	match request {
		UiFrontendRequest::ShowQuestion(_) => Ok(UiFrontendReply::ShowQuestionReply(serde_json::from_value(reply)?)),
		UiFrontendRequest::ShowDialog(_) => Ok(UiFrontendReply::ShowDialogReply()),
		UiFrontendRequest::DebugSleep(_) => Ok(UiFrontendReply::DebugSleepReply()),
		UiFrontendRequest::WorkspaceFolder => Ok(UiFrontendReply::WorkspaceFolderReply(serde_json::from_value(reply)?)),
		UiFrontendRequest::LastActiveEditorContext => Ok(UiFrontendReply::LastActiveEditorContextReply(serde_json::from_value(reply)?)),
	}
}

