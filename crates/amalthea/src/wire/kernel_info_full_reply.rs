/*
 * kernel_info_full_reply.rs
 *
 * Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::help_link::HelpLink;
use crate::wire::jupyter_message::MessageType;
use crate::wire::jupyter_message::Status;
use crate::wire::kernel_info_reply;
use crate::wire::language_info::LanguageInfo;

/// Complete version of `kernel_info_reply`
///
/// Private to Amalthea. Includes fields owned by Amalthea such as the protocol
/// version and feature flags.
///
/// Kernel authors should use [kernel_info_reply::KernelInfoReply] instead.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct KernelInfoReply {
    /// Version of messaging protocol
    pub protocol_version: String,

    /// List of feature flags supported by the kernel. See JEP 92.
    pub supported_features: Vec<String>,

    /// The execution status ("ok" or "error")
    pub status: Status,

    /// Information about the language the kernel supports
    pub language_info: LanguageInfo,

    /// A startup banner
    pub banner: String,

    /// Whether debugging is supported
    pub debugger: bool,

    /// A list of help links
    pub help_links: Vec<HelpLink>,
}

impl MessageType for KernelInfoReply {
    fn message_type() -> String {
        String::from("kernel_info_reply")
    }
}

/// Adds Amalthea fields to partial [kernel_info_reply::KernelInfoReply].
impl From<kernel_info_reply::KernelInfoReply> for KernelInfoReply {
    fn from(value: kernel_info_reply::KernelInfoReply) -> Self {
        Self {
            // These fields are set by Amalthea
            protocol_version: String::from("5.4"),
            supported_features: vec![String::from("iopub_welcome")],

            // These fields are set by the Amalthea user
            status: value.status,
            language_info: value.language_info,
            banner: value.banner,
            debugger: value.debugger,
            help_links: value.help_links,
        }
    }
}
