/*
 * comm_event.rs
 *
 * Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
 *
 */

use crossbeam::channel::Sender;
use serde_json::Value;

use crate::comm::comm_channel::CommMsg;
use crate::socket::comm::CommSocket;

/// Comm events sent to the frontend via Shell.
pub enum CommEvent {
    /// A new Comm was opened. The optional `Sender` is a synchronisation
    /// barrier: if provided, Shell signals it after processing the open
    /// (sending `comm_open` on IOPub). The caller blocks on the paired
    /// receiver to guarantee that the `comm_open` message has been sent
    /// before any subsequent messages.
    Opened(CommSocket, Value, Option<Sender<()>>),

    /// A message was received on a Comm; the first value is the comm ID, and the
    /// second value is the message.
    Message(String, CommMsg),

    /// A Comm was closed
    Closed(String),
}
