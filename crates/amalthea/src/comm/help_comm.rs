/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from help.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// Possible values for Kind in ShowHelp
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ShowHelpKind {
	#[serde(rename = "html")]
	Html,

	#[serde(rename = "markdown")]
	Markdown,

	#[serde(rename = "url")]
	Url
}

/// Parameters for the ShowHelpTopic method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowHelpTopicParams {
	/// The help topic to show
	pub topic: String,
}

/// Parameters for the ShowHelp method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowHelpParams {
	/// The help content to show
	pub content: String,

	/// The type of content to show
	pub kind: ShowHelpKind,

	/// Whether to focus the Help pane when the content is displayed.
	pub focus: bool,
}

/**
 * Backend RPC request types for the help comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum HelpBackendRpcRequest {
	/// Look for and, if found, show a help topic.
	///
	/// Requests that the help backend look for a help topic and, if found,
	/// show it. If the topic is found, it will be shown via a Show Help
	/// notification. If the topic is not found, no notification will be
	/// delivered.
	#[serde(rename = "show_help_topic")]
	ShowHelpTopic(ShowHelpTopicParams),

}

/**
 * Backend RPC Reply types for the help comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum HelpBackendRpcReply {
	/// Whether the topic was found and shown. Topics are shown via a Show
	/// Help notification.
	ShowHelpTopicReply(bool),

}

/**
 * Frontend RPC request types for the help comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum HelpFrontendRpcRequest {
}

/**
 * Frontend RPC Reply types for the help comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum HelpFrontendRpcReply {
}

/**
 * Frontend events for the help comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum HelpEvent {
	#[serde(rename = "show_help")]
	ShowHelp(ShowHelpParams),

}

