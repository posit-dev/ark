/*
 * help_link.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use serde::Serialize;

/// Represents a help link in a Jupyter message
#[derive(Serialize)]
pub struct HelpLink {
    /// The text to display for the link
    pub text: String,

    /// The location (URL) of the help link
    pub url: String,
}
