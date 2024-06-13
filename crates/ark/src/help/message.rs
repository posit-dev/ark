//
// message.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

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

impl std::fmt::Display for HelpRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            HelpRequest::ShowHelpUrlRequest(_) => write!(f, "ShowHelpUrlRequest"),
        }
    }
}
