// @generated

/*---------------------------------------------------------------------------------------------
 *  Copyright (C) 2024-2026 Posit Software, PBC. All rights reserved.
 *--------------------------------------------------------------------------------------------*/

//
// AUTO-GENERATED from help.json; do not edit.
//

use serde::Deserialize;
use serde::Serialize;

/// A help topic offered as an autocomplete suggestion.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HelpTopicSuggestion {
	/// The topic label shown to the user.
	pub label: String,

	/// The exact topic value used to open help.
	pub topic: String,

	/// Optional context such as the package containing the topic.
	pub detail: Option<String>
}

/// Possible values for Kind in ShowHelp
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, strum_macros::Display, strum_macros::EnumString)]
pub enum ShowHelpKind {
	#[serde(rename = "html")]
	#[strum(to_string = "html")]
	Html,

	#[serde(rename = "markdown")]
	#[strum(to_string = "markdown")]
	Markdown,

	#[serde(rename = "url")]
	#[strum(to_string = "url")]
	Url
}

/// Parameters for the ShowHelpTopic method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ShowHelpTopicParams {
	/// The help topic to show
	pub topic: String,
}

/// Parameters for the SearchHelp method.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct SearchHelpParams {
	/// The help query to search for
	pub query: String,
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
pub enum HelpBackendRequest {
	/// Look for and, if found, show a help topic.
	///
	/// Requests that the help backend look for a help topic and, if found,
	/// show it. If the topic is found, it will be shown via a Show Help
	/// notification. If the topic is not found, no notification will be
	/// delivered.
	#[serde(rename = "show_help_topic")]
	ShowHelpTopic(ShowHelpTopicParams),

	/// Search the active interpreter's help system.
	///
	/// Searches interpreter-wide help and displays the resulting page via a
	/// Show Help notification.
	#[serde(rename = "search_help")]
	SearchHelp(SearchHelpParams),

	/// List help topics for autocomplete.
	///
	/// Returns interpreter-wide help topics that can be offered as search
	/// suggestions.
	#[serde(rename = "get_help_topics")]
	GetHelpTopics,

}

/**
 * Backend RPC Reply types for the help comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum HelpBackendReply {
	/// Whether the topic was found and shown. Topics are shown via a Show
	/// Help notification.
	ShowHelpTopicReply(bool),

	/// Whether the search results page was shown.
	SearchHelpReply(bool),

	/// Help topic suggestions.
	GetHelpTopicsReply(Vec<HelpTopicSuggestion>),

}

/**
 * Frontend RPC request types for the help comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum HelpFrontendRequest {
}

/**
 * Frontend RPC Reply types for the help comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "result")]
pub enum HelpFrontendReply {
}

/**
 * Frontend events for the help comm
 */
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "method", content = "params")]
pub enum HelpFrontendEvent {
	#[serde(rename = "show_help")]
	ShowHelp(ShowHelpParams),

}

