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
    ShowHelpTopic(HelpMessageShowTopic),

    /// Notify the front end of new content in the Help pane.
    ShowHelp(HelpMessageShowHelp),
}

/// Request to show a help topic in the Help pane.
#[derive(Debug, Serialize, Deserialize)]
pub struct HelpMessageShowTopic {
    /// The help topic to be shown.
    pub topic: String,
}

/// Show help content in the Help pane.
#[derive(Debug, Serialize, Deserialize)]
pub struct HelpMessageShowHelp {
    /// The help content to be shown.
    pub content: String,

    /// The content help type. Must be one of 'html', 'markdown', or 'url'.
    pub kind: String,

    /// Focus the Help pane after the Help content has been rendered?
    pub focus: bool,
}
