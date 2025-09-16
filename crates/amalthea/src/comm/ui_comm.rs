// @generated

/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024-2025 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from ui.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;
use super::plot_comm::PlotRenderSettings;

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

/// Selection range
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Range {
	/// Start position of the selection
	pub start: Position,

	/// End position of the selection
	pub end: Position
}

/// Possible values for Kind in OpenEditor
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, strum_macros::Display, strum_macros::EnumString)]
pub enum OpenEditorKind {
	#[serde(rename = "path")]
	#[strum(to_string = "path")]
	Path,

	#[serde(rename = "uri")]
	#[strum(to_string = "uri")]
	Uri
}

/// Parameters for the DidChangePlotsRenderSettings method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DidChangePlotsRenderSettingsParams {
	/// Plot rendering settings.
	pub settings: PlotRenderSettings,
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

	/// How to interpret the 'file' argument: as a file path or as a URI. If
	/// omitted, defaults to 'path'.
	pub kind: OpenEditorKind,
}

/// Parameters for the NewDocument method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NewDocumentParams {
	/// Document contents
	pub contents: String,

	/// Language identifier
	pub language_id: String,
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

/// Parameters for the ShowPrompt method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowPromptParams {
	/// The title of the prompt dialog, such as 'Enter Swallow Velocity'
	pub title: String,

	/// The message prompting the user for text, such as 'What is the airspeed
	/// velocity of an unladen swallow?'
	pub message: String,

	/// The default value with which to pre-populate the text input box, such
	/// as 'African or European?'
	pub default: String,

	/// The number of seconds to wait for the user to reply before giving up.
	pub timeout: i64,
}

/// Parameters for the AskForPassword method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AskForPasswordParams {
	/// The prompt, such as 'Please enter your password'
	pub prompt: String,
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

/// Parameters for the EvaluateWhenClause method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EvaluateWhenClauseParams {
	/// The values for context keys, as a `when` clause
	pub when_clause: String,
}

/// Parameters for the ExecuteCode method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ExecuteCodeParams {
	/// The language ID of the code to execute
	pub language_id: String,

	/// The code to execute
	pub code: String,

	/// Whether to focus the runtime's console
	pub focus: bool,

	/// Whether to bypass runtime code completeness checks
	pub allow_incomplete: bool,
}

/// Parameters for the OpenWorkspace method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OpenWorkspaceParams {
	/// The path for the workspace to be opened
	pub path: String,

	/// Should the workspace be opened in a new window?
	pub new_window: bool,
}

/// Parameters for the SetEditorSelections method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SetEditorSelectionsParams {
	/// The selections (really, ranges) to set in the document
	pub selections: Vec<Range>,
}

/// Parameters for the ModifyEditorSelections method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ModifyEditorSelectionsParams {
	/// The selections (really, ranges) to set in the document
	pub selections: Vec<Range>,

	/// The text values to insert at the selections
	pub values: Vec<String>,
}

/// Parameters for the ShowUrl method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowUrlParams {
	/// The URL to display
	pub url: String,
}

/// Parameters for the ShowHtmlFile method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowHtmlFileParams {
	/// The fully qualified filesystem path to the HTML file to display
	pub path: String,

	/// A title to be displayed in the viewer. May be empty, and can be
	/// superseded by the title in the HTML file.
	pub title: String,

	/// Whether the HTML file is a plot-like object
	pub is_plot: bool,

	/// The desired height of the HTML viewer, in pixels. The special value 0
	/// indicates that no particular height is desired, and -1 indicates that
	/// the viewer should be as tall as possible.
	pub height: i64,
}

/// Parameters for the OpenWithSystem method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct OpenWithSystemParams {
	/// The file path to open with the system default application
	pub path: String,
}

/**
 * Backend RPC request types for the ui comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum UiBackendRequest {
	/// Notification that the settings to render a plot (i.e. the plot size)
	/// have changed.
	///
	/// Typically fired when the plot component has been resized by the user.
	/// This notification is useful to produce accurate pre-renderings of
	/// plots.
	#[serde(rename = "did_change_plots_render_settings")]
	DidChangePlotsRenderSettings(DidChangePlotsRenderSettingsParams),

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
	/// Unused response to notification
	DidChangePlotsRenderSettingsReply(),

	/// The method result
	CallMethodReply(CallMethodResult),

}

/**
 * Frontend RPC request types for the ui comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum UiFrontendRequest {
	/// Create a new document with text contents
	///
	/// Use this to create a new document with the given language ID and text
	/// contents
	#[serde(rename = "new_document")]
	NewDocument(NewDocumentParams),

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

	/// Show a prompt
	///
	/// Use this for an input box where user can input any string
	#[serde(rename = "show_prompt")]
	ShowPrompt(ShowPromptParams),

	/// Ask the user for a password
	///
	/// Use this for an input box where the user can input a password
	#[serde(rename = "ask_for_password")]
	AskForPassword(AskForPasswordParams),

	/// Sleep for n seconds
	///
	/// Useful for testing in the backend a long running frontend method
	#[serde(rename = "debug_sleep")]
	DebugSleep(DebugSleepParams),

	/// Execute a Positron command
	///
	/// Use this to execute a Positron command from the backend (like from a
	/// runtime), and wait for the command to finish
	#[serde(rename = "execute_command")]
	ExecuteCommand(ExecuteCommandParams),

	/// Get a logical for a `when` clause (a set of context keys)
	///
	/// Use this to evaluate a `when` clause of context keys in the frontend
	#[serde(rename = "evaluate_when_clause")]
	EvaluateWhenClause(EvaluateWhenClauseParams),

	/// Execute code in a Positron runtime
	///
	/// Use this to execute code in a Positron runtime
	#[serde(rename = "execute_code")]
	ExecuteCode(ExecuteCodeParams),

	/// Path to the workspace folder
	///
	/// Returns the path to the workspace folder, or first folder if there are
	/// multiple.
	#[serde(rename = "workspace_folder")]
	WorkspaceFolder,

	/// Modify selections in the editor with a text edit
	///
	/// Use this to edit a set of selection ranges/cursor in the editor
	#[serde(rename = "modify_editor_selections")]
	ModifyEditorSelections(ModifyEditorSelectionsParams),

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
	/// Reply for the new_document method (no result)
	NewDocumentReply(),

	/// Whether the user accepted or rejected the dialog.
	ShowQuestionReply(bool),

	/// Reply for the show_dialog method (no result)
	ShowDialogReply(),

	/// The input from the user
	ShowPromptReply(Option<String>),

	/// The input from the user
	AskForPasswordReply(Option<String>),

	/// Reply for the debug_sleep method (no result)
	DebugSleepReply(),

	/// Reply for the execute_command method (no result)
	ExecuteCommandReply(),

	/// Whether the `when` clause evaluates as true or false
	EvaluateWhenClauseReply(bool),

	/// Reply for the execute_code method (no result)
	ExecuteCodeReply(),

	/// The path to the workspace folder
	WorkspaceFolderReply(Option<String>),

	/// Reply for the modify_editor_selections method (no result)
	ModifyEditorSelectionsReply(),

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

	/// Use this to open a workspace in Positron
	#[serde(rename = "open_workspace")]
	OpenWorkspace(OpenWorkspaceParams),

	/// Use this to set the selection ranges/cursor in the editor
	#[serde(rename = "set_editor_selections")]
	SetEditorSelections(SetEditorSelectionsParams),

	/// Causes the URL to be displayed inside the Viewer pane, and makes the
	/// Viewer pane visible.
	#[serde(rename = "show_url")]
	ShowUrl(ShowUrlParams),

	/// Causes the HTML file to be shown in Positron.
	#[serde(rename = "show_html_file")]
	ShowHtmlFile(ShowHtmlFileParams),

	/// Open a file or folder with the system default application
	#[serde(rename = "open_with_system")]
	OpenWithSystem(OpenWithSystemParams),

	/// This event is used to signal that the stored messages the front-end
	/// replays when constructing multi-output plots should be reset. This
	/// happens for things like a holoviews extension being changed.
	#[serde(rename = "clear_webview_preloads")]
	ClearWebviewPreloads,

}

/**
* Conversion of JSON values to frontend RPC Reply types
*/
pub fn ui_frontend_reply_from_value(
	reply: serde_json::Value,
	request: &UiFrontendRequest,
) -> anyhow::Result<UiFrontendReply> {
	match request {
		UiFrontendRequest::NewDocument(_) => Ok(UiFrontendReply::NewDocumentReply()),
		UiFrontendRequest::ShowQuestion(_) => Ok(UiFrontendReply::ShowQuestionReply(serde_json::from_value(reply)?)),
		UiFrontendRequest::ShowDialog(_) => Ok(UiFrontendReply::ShowDialogReply()),
		UiFrontendRequest::ShowPrompt(_) => Ok(UiFrontendReply::ShowPromptReply(serde_json::from_value(reply)?)),
		UiFrontendRequest::AskForPassword(_) => Ok(UiFrontendReply::AskForPasswordReply(serde_json::from_value(reply)?)),
		UiFrontendRequest::DebugSleep(_) => Ok(UiFrontendReply::DebugSleepReply()),
		UiFrontendRequest::ExecuteCommand(_) => Ok(UiFrontendReply::ExecuteCommandReply()),
		UiFrontendRequest::EvaluateWhenClause(_) => Ok(UiFrontendReply::EvaluateWhenClauseReply(serde_json::from_value(reply)?)),
		UiFrontendRequest::ExecuteCode(_) => Ok(UiFrontendReply::ExecuteCodeReply()),
		UiFrontendRequest::WorkspaceFolder => Ok(UiFrontendReply::WorkspaceFolderReply(serde_json::from_value(reply)?)),
		UiFrontendRequest::ModifyEditorSelections(_) => Ok(UiFrontendReply::ModifyEditorSelectionsReply()),
		UiFrontendRequest::LastActiveEditorContext => Ok(UiFrontendReply::LastActiveEditorContextReply(serde_json::from_value(reply)?)),
	}
}

