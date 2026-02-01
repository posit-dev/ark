//
// accumulator.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

//! Message accumulator for resilient IOPub message matching in tests.
//!
//! Stream messages from R can be batched (one message containing multiple outputs) or
//! split (multiple messages), depending on timing. This module provides a
//! `MessageAccumulator` that automatically coalesces stream fragments with the same
//! parent header, making tests immune to batching variations.

use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use amalthea::socket::socket::Socket;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::status::ExecutionState;
use amalthea::wire::stream::Stream;

/// Accumulates IOPub messages and coalesces stream fragments.
///
/// Stream messages with the same parent header are automatically combined,
/// eliminating sensitivity to whether R batched or split the output.
pub struct MessageAccumulator {
    /// All received messages (for diagnostics)
    messages: Vec<Message>,
    /// Coalesced stdout streams keyed by parent message ID
    stdout_streams: HashMap<String, String>,
    /// Coalesced stderr streams keyed by parent message ID
    stderr_streams: HashMap<String, String>,
    /// Whether we've seen an idle status
    saw_idle: bool,
    /// Comm methods we've seen (e.g., "start_debug", "stop_debug")
    comm_methods: Vec<String>,
}

impl MessageAccumulator {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            stdout_streams: HashMap::new(),
            stderr_streams: HashMap::new(),
            saw_idle: false,
            comm_methods: Vec::new(),
        }
    }

    /// Receive messages until the condition is satisfied or timeout.
    ///
    /// The condition function is called after each message is accumulated,
    /// allowing it to check for complex conditions across multiple messages.
    ///
    /// Returns `Ok(())` if the condition was satisfied, or `Err` with a
    /// diagnostic message if the timeout was reached.
    pub fn receive_until<F>(
        &mut self,
        socket: &Socket,
        mut condition: F,
        timeout: Duration,
    ) -> Result<(), String>
    where
        F: FnMut(&Self) -> bool,
    {
        let start = Instant::now();
        let poll_timeout_ms = 100;

        loop {
            if condition(self) {
                return Ok(());
            }

            if start.elapsed() >= timeout {
                return Err(format!(
                    "Timeout after {:?} waiting for condition.\n\
                     Accumulated {} messages.\n\
                     Coalesced stdout streams: {:?}\n\
                     Coalesced stderr streams: {:?}\n\
                     Comm methods seen: {:?}\n\
                     Saw idle: {}\n\
                     Raw messages: {:#?}",
                    timeout,
                    self.messages.len(),
                    self.stdout_streams,
                    self.stderr_streams,
                    self.comm_methods,
                    self.saw_idle,
                    self.messages
                ));
            }

            // Poll for incoming message
            match socket.poll_incoming(poll_timeout_ms) {
                Ok(false) => continue,
                Ok(true) => {},
                Err(e) => {
                    return Err(format!(
                        "Error polling socket: {:?}\n\
                         Accumulated so far: {:#?}",
                        e, self.messages
                    ));
                },
            }

            let msg = match Message::read_from_socket(socket) {
                Ok(msg) => msg,
                Err(e) => {
                    return Err(format!(
                        "Error reading message: {:?}\n\
                         Accumulated so far: {:#?}",
                        e, self.messages
                    ));
                },
            };

            self.accumulate(msg);
        }
    }

    /// Accumulate a single message, updating internal state.
    fn accumulate(&mut self, msg: Message) {
        match &msg {
            Message::Stream(stream) => {
                // Key by parent_header.msg_id if present, otherwise use the
                // stream message's own header.msg_id to avoid collapsing
                // unrelated orphan streams into the same bucket.
                let key = stream
                    .parent_header
                    .as_ref()
                    .map(|h| &h.msg_id)
                    .unwrap_or(&stream.header.msg_id)
                    .clone();

                let text = &stream.content.text;
                let streams = match stream.content.name {
                    Stream::Stdout => &mut self.stdout_streams,
                    Stream::Stderr => &mut self.stderr_streams,
                };

                streams.entry(key).or_default().push_str(text);
            },

            Message::CommMsg(comm) => {
                if let Some(method) = comm.content.data.get("method").and_then(|m| m.as_str()) {
                    self.comm_methods.push(method.to_string());
                }
            },

            Message::Status(status) => {
                if status.content.execution_state == ExecutionState::Idle {
                    self.saw_idle = true;
                }
            },

            _ => {},
        }

        self.messages.push(msg);
    }

    /// Check if any coalesced stdout stream contains the given text.
    ///
    /// This checks across all parent headers, so it works regardless of
    /// whether the text came from multiple batched outputs or a single one.
    pub fn stdout_contains(&self, text: &str) -> bool {
        self.stdout_streams.values().any(|s| s.contains(text))
    }

    /// Check if any coalesced stderr stream contains the given text.
    pub fn stderr_contains(&self, text: &str) -> bool {
        self.stderr_streams.values().any(|s| s.contains(text))
    }

    /// Check if any stream (stdout or stderr) contains the given text.
    pub fn streams_contain(&self, text: &str) -> bool {
        self.stdout_contains(text) || self.stderr_contains(text)
    }

    /// Check if we've seen a comm message with the given method.
    pub fn has_comm_method(&self, method: &str) -> bool {
        self.comm_methods.iter().any(|m| m == method)
    }

    /// Check if we've seen at least N comm messages with the given method.
    pub fn has_comm_method_count(&self, method: &str, count: usize) -> bool {
        self.comm_methods.iter().filter(|m| *m == method).count() >= count
    }

    /// Check if we've seen an idle status message.
    pub fn saw_idle(&self) -> bool {
        self.saw_idle
    }

    /// Get all accumulated messages (for diagnostics).
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Get the coalesced stdout for all parent headers combined.
    pub fn all_stdout(&self) -> String {
        self.stdout_streams.values().cloned().collect()
    }

    /// Get the coalesced stderr for all parent headers combined.
    pub fn all_stderr(&self) -> String {
        self.stderr_streams.values().cloned().collect()
    }

    /// Get the number of messages accumulated.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Drain any remaining messages with a short timeout.
    ///
    /// This is useful after the condition is satisfied to clean up
    /// any messages that might interfere with subsequent operations.
    pub fn drain(&mut self, socket: &Socket, timeout_ms: i64) {
        loop {
            match socket.poll_incoming(timeout_ms) {
                Ok(true) => {
                    if let Ok(msg) = Message::read_from_socket(socket) {
                        self.accumulate(msg);
                    }
                },
                // No more messages or error - stop draining
                Ok(false) | Err(_) => break,
            }
        }
    }
}

impl Default for MessageAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_accumulator_initial_state() {
        let acc = MessageAccumulator::new();
        assert!(!acc.saw_idle());
        assert!(!acc.streams_contain("anything"));
        assert!(!acc.has_comm_method("start_debug"));
        assert_eq!(acc.message_count(), 0);
    }
}
