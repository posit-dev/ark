/*
 * kernel_info_reply.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::help_link::HelpLink;
use crate::wire::jupyter_message::MessageType;
use crate::wire::language_info::LanguageInfo;
use serde::{Deserialize, Serialize};

/// Represents a reply to a kernel_info_request
#[derive(Debug, Serialize, Deserialize)]
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

impl MessageType for KernelInfoReply {
    fn message_type() -> String {
        String::from("kernel_info_reply")
    }
}
