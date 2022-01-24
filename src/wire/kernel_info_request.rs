/*
 * kernel_info_request.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use serde::Deserialize;

/// Represents request from the front end to the kernel to get information
#[derive(Deserialize)]
pub struct KernelInfoRequest {}

impl MessageType for KernelInfoRequest {
    fn message_type() -> String {
        String::from("kernel_info_request")
    }
}
