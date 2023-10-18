//
// message.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use serde::Deserialize;
use serde::Serialize;

/**
 * Enum representing the different types of messages that can be sent over the
 * Help comm channel and their associated data.
 */
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "msg_type", rename_all = "snake_case")]
pub enum HelpMessage {
    /// Request from the front end to show a help topic in the Help pane.
    ShowHelpTopicRequest(ShowTopicRequest),

    /// Reply to ShowHelpTopicRequest.
    ShowHelpTopicReply(ShowTopicReply),

    /// Notify the front end of new content in the Help pane.
    ShowHelpEvent(ShowHelpContent),
}

/**
 * Enum representing requests for the Help thread from other threads.
 */
#[derive(Debug)]
pub enum HelpRequest {
    /// Request to show the given URL to the user in the Help pane.
    ShowHelpUrlRequest(String),
}

/**
 * Enum representing replies from the Help thread.
 */
pub enum HelpReply {
    /// Reply to ShowHelpUrlRequest; indicates whether the URL was successfully
    /// shown.
    ShowHelpUrlReply(bool),
}

/// Request to show a help topic in the Help pane.
#[derive(Debug, Serialize, Deserialize)]
pub struct ShowTopicRequest {
    /// The help topic to be shown.
    pub topic: String,
}

/// Reply to a request to show a help topic in the Help pane.
#[derive(Debug, Serialize, Deserialize)]
pub struct ShowTopicReply {
    /// Whether or not the topic was found.
    pub found: bool,
}

/// Show help content in the Help pane.
#[derive(Debug, Serialize, Deserialize)]
pub struct ShowHelpContent {
    /// The help content to be shown.
    pub content: String,

    /// The content help type. Must be one of 'html', 'markdown', or 'url'.
    pub kind: String,

    /// Focus the Help pane after the Help content has been rendered?
    pub focus: bool,
}
