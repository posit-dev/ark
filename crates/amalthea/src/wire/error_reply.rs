/*
 * error_reply.rs
 *
 * Copyright (C) 2022 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::wire::exception::Exception;
use crate::wire::jupyter_message::MessageType;
use crate::wire::jupyter_message::Status;

/// Represents an error that occurred after processing a request on a
/// ROUTER/DEALER socket.
///
/// This is the payload of a response to a request. Note that, as an exception,
/// responses to `"execute_request"` include an `execution_count` field. We
/// represent these with an `ExecuteReplyException`.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ErrorReply {
    /// The status; always Error
    pub status: Status,

    /// The exception that occurred during execution
    #[serde(flatten)]
    pub exception: Exception,
}

/// Note that the message type of an error reply is generally adjusted to match
/// its request type (e.g. foo_request => foo_reply). The message type
/// implemented here is only a placeholder and should not appear in any
/// serialized/deserialized message.
impl MessageType for ErrorReply {
    fn message_type() -> String {
        String::from("*error payload*")
    }
}
