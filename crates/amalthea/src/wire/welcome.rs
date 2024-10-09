/*
 * welcome.rs
 *
 * Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::jupyter_message::MessageType;

/// An IOPub message used for handshaking by modern clients.
/// See JEP 65: https://github.com/jupyter/enhancement-proposals/pull/65
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Welcome {
    pub subscription: String,
}

impl MessageType for Welcome {
    fn message_type() -> String {
        String::from("welcome")
    }
}
