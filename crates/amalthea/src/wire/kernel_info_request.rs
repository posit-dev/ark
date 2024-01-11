/*
 * kernel_info_request.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// Represents request from the frontend to the kernel to get information
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct KernelInfoRequest {}

impl MessageType for KernelInfoRequest {
    fn message_type() -> String {
        String::from("kernel_info_request")
    }
}
