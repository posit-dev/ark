/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2023 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from help.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowHelpTopicParams {
	/// The help topic to show
	pub topic: String,

}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowHelpParams {
	/// The help content to show
	pub content: String,

	/// The type of content to show
	pub kind: String,

	/// Whether to focus the Help pane when the content is displayed.
	pub focus: bool,

}

/**
 * RPC request types for the help comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum HelpRpcRequest {
	/// Look for and, if found, show a help topic.
	#[serde(rename = "show_help_topic")]
	ShowHelpTopic(ShowHelpTopicParams),
}

/**
 * RPC Reply types for the help comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum HelpRpcReply {
	/// Whether the topic was found and shown. Topics are shown via a Show
	/// Help notification.
	ShowHelpTopicReply(bool),
}

/**
 * Front-end events for the help comm
 */
#[derive(Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum HelpEvent {
	#[serde(rename = "show_help")]
	ShowHelp(ShowHelpParams),
}

