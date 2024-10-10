/*
 * subscription_message.rs
 *
 * Copyright (C) 2024 Posit Software, PBC. All rights reserved.
 *
 */

use serde::Deserialize;
use serde::Serialize;

use crate::error::Error;
use crate::socket::socket::Socket;

/// Represents a special `SubscriptionMessage` sent from a SUB to an XPUB
/// upon `socket.set_subscribe(subscription)` or `socket.set_unsubscribe(subscription)`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SubscriptionMessage {
    pub kind: SubscriptionKind,
    pub subscription: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum SubscriptionKind {
    Subscribe,
    Unsubscribe,
}

impl SubscriptionMessage {
    /// Read a SubscriptionMessage from a ZeroMQ socket.
    pub fn read_from_socket(socket: &Socket) -> crate::Result<SubscriptionMessage> {
        let bufs = socket.recv_multipart()?;
        Self::from_buffers(bufs)
    }

    /// Parse a SubscriptionMessage from an array of buffers (from a ZeroMQ message)
    ///
    /// Always a single frame (i.e. `bufs` should be length 1).
    /// Either `1{subscription}` for subscription.
    /// Or `0{subscription}` for unsubscription.
    fn from_buffers(bufs: Vec<Vec<u8>>) -> crate::Result<SubscriptionMessage> {
        if bufs.len() != 1 {
            let n = bufs.len();
            return Err(crate::anyhow!(
                "Subscription message on XPUB must be a single frame. {n} frames were received."
            ));
        }

        let buf = bufs.get(0).unwrap();

        if buf.len() == 0 {
            return Err(crate::anyhow!(
                "Subscription message on XPUB must be at least length 1 to determine subscribe/unsubscribe."
            ));
        }

        let kind = if buf[0] == 1 {
            SubscriptionKind::Subscribe
        } else {
            SubscriptionKind::Unsubscribe
        };

        // Advance to access remaining buffer
        let buf = &buf[1..];

        // The rest of the message is the UTF-8 `subscription`
        let subscription = match std::str::from_utf8(&buf) {
            Ok(subscription) => subscription,
            Err(err) => {
                return Err(Error::Utf8Error(
                    String::from("subscription"),
                    buf.to_vec(),
                    err,
                ))
            },
        };

        let subscription = subscription.to_string();

        Ok(Self { kind, subscription })
    }
}
