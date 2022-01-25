/*
 * kernel_info_request.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::wire::jupyter_message::MessageType;
use serde::Deserialize;

/// Represents request from the front end to the kernel to get information
#[derive(Debug, Deserialize)]
pub struct KernelInfoRequest {}

impl MessageType for KernelInfoRequest {
    fn message_type() -> String {
        String::from("kernel_info_request")
    }
}
