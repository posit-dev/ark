/*
 * kernel_info_reply.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::help_link::HelpLink;
use crate::wire::language_info::LanguageInfo;
use serde::Serialize;

/// Represents a reply to a kernel_info_request
#[derive(Serialize)]
pub struct KernelInfoReply {
    /// The execution status ("ok" or "error")
    status: String,

    /// Version of messaging protocol
    protocol_version: String,

    /// Information about the language the kernel supports
    language_info: LanguageInfo,

    /// A startup banner
    banner: String,

    /// Whether debugging is supported
    debugger: bool,

    /// A list of help links
    help_links: Vec<HelpLink>,
}
