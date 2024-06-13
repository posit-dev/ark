//
// message.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

/**
 * Enum representing events for the Help thread from other threads.
 */
#[derive(Debug)]
pub enum HelpEvent {
    /// Event to show the given URL to the user in the Help pane. Accomplished by
    /// forwarding the URL on to the frontend using `HelpFrontendEvent::ShowHelp`.
    ShowHelpUrl(ShowHelpUrlParams),
}

#[derive(Debug)]
pub struct ShowHelpUrlParams {
    /// Url to attempt to show.
    pub url: String,
}

impl std::fmt::Display for HelpEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            HelpEvent::ShowHelpUrl(_) => write!(f, "ShowHelpUrl"),
        }
    }
}
