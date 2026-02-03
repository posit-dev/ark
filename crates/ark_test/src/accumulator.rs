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
use std::collections::HashSet;
use std::time::Duration;
use std::time::Instant;

/// Time to wait for trailing stream messages after condition is met.
/// Stream messages can arrive slightly out of order due to batching nondeterminism.
const SETTLE_TIMEOUT_MS: i64 = 50;

use amalthea::socket::socket::Socket;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::status::ExecutionState;
use amalthea::wire::stream::Stream;

use crate::tracing::trace_iopub_message;
use crate::tracing::IoPubTrace;

/// Accumulates IOPub messages and coalesces stream fragments.
///
/// Stream messages with the same parent header are automatically combined,
/// eliminating sensitivity to whether R batched or split the output.
pub struct MessageAccumulator {
    /// All received messages
    pub messages: Vec<Message>,
    /// Coalesced stdout streams keyed by parent message ID
    pub stdout_streams: HashMap<String, String>,
    /// Coalesced stderr streams keyed by parent message ID
    pub stderr_streams: HashMap<String, String>,
    /// Indices of messages that have been explicitly checked/consumed
    consumed: HashSet<usize>,
    /// Whether we've seen an idle status
    saw_idle: bool,
    /// Whether receive_until completed successfully (enables Drop check)
    verified: bool,
}

impl MessageAccumulator {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            consumed: HashSet::new(),
            stdout_streams: HashMap::new(),
            stderr_streams: HashMap::new(),
            saw_idle: false,
            verified: false,
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
        F: FnMut(&mut Self) -> bool,
    {
        let start = Instant::now();
        let poll_timeout_ms = 100;

        loop {
            if condition(self) {
                // Condition met. Allow a short settling period for any trailing
                // stream messages that may arrive due to batching nondeterminism.
                self.settle(socket, SETTLE_TIMEOUT_MS);
                self.verified = true;
                return Ok(());
            }

            if start.elapsed() >= timeout {
                return Err(format!(
                    "Timeout after {timeout:?} waiting for condition.\n\
                     Accumulated {} messages.\n\
                     Coalesced stdout streams: {:?}\n\
                     Coalesced stderr streams: {:?}\n\
                     Saw idle: {}\n\
                     Raw messages: {:#?}",
                    self.messages.len(),
                    self.stdout_streams,
                    self.stderr_streams,
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

    /// Wait briefly for any trailing messages (primarily streams) that may
    /// arrive after the condition is met due to batching nondeterminism.
    fn settle(&mut self, socket: &Socket, timeout_ms: i64) {
        loop {
            match socket.poll_incoming(timeout_ms) {
                Ok(true) => {
                    if let Ok(msg) = Message::read_from_socket(socket) {
                        self.accumulate(msg);
                    }
                },
                Ok(false) | Err(_) => break,
            }
        }
    }

    fn accumulate(&mut self, msg: Message) {
        self.trace_message(&msg);

        match &msg {
            Message::Stream(stream) => {
                // Key by `parent_header.msg_id` if present, otherwise use the
                // stream message's own `header.msg_id` to avoid collapsing
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

            Message::Status(status) => {
                if status.content.execution_state == ExecutionState::Idle {
                    self.saw_idle = true;
                }
            },

            _ => {},
        }

        self.messages.push(msg);
    }

    /// Trace a message for debugging (enable with `ARK_TEST_TRACE=1`)
    fn trace_message(&self, msg: &Message) {
        let trace = match msg {
            Message::Status(status) => match status.content.execution_state {
                ExecutionState::Busy => IoPubTrace::Busy,
                ExecutionState::Idle => IoPubTrace::Idle,
                ExecutionState::Starting => IoPubTrace::Status {
                    state: "starting".to_string(),
                },
            },
            Message::ExecuteInput(input) => IoPubTrace::ExecuteInput {
                code: input.content.code.clone(),
            },
            Message::ExecuteResult(_) => IoPubTrace::ExecuteResult,
            Message::ExecuteError(err) => IoPubTrace::ExecuteError {
                message: err.content.exception.evalue.clone(),
            },
            Message::Stream(stream) => {
                let name = match stream.content.name {
                    Stream::Stdout => "stdout",
                    Stream::Stderr => "stderr",
                };
                IoPubTrace::Stream {
                    name: name.to_string(),
                    text: stream.content.text.clone(),
                }
            },
            Message::CommOpen(comm) => IoPubTrace::CommOpen {
                target: comm.content.target_name.clone(),
            },
            Message::CommMsg(comm) => {
                let method = comm
                    .content
                    .data
                    .get("method")
                    .and_then(|m| m.as_str())
                    .unwrap_or("?")
                    .to_string();
                IoPubTrace::CommMsg { method }
            },
            Message::CommClose(_) => IoPubTrace::CommClose,
            _ => IoPubTrace::Other {
                msg_type: format!("{:?}", std::mem::discriminant(msg)),
            },
        };
        trace_iopub_message(&trace);
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

    /// Find all messages matching a predicate (without marking as consumed).
    pub fn find<'a, F>(&'a self, predicate: F) -> impl Iterator<Item = &'a Message>
    where
        F: Fn(&Message) -> bool + 'a,
    {
        self.messages.iter().filter(move |m| predicate(m))
    }

    /// Check if any message matches a predicate (without marking as consumed).
    pub fn any<F>(&self, predicate: F) -> bool
    where
        F: Fn(&Message) -> bool,
    {
        self.messages.iter().any(predicate)
    }

    /// Mark messages matching a predicate as consumed and return matching count.
    pub fn consume<F>(&mut self, predicate: F) -> usize
    where
        F: Fn(&Message) -> bool,
    {
        let mut count = 0;
        for (i, msg) in self.messages.iter().enumerate() {
            if predicate(msg) {
                self.consumed.insert(i);
                count += 1;
            }
        }
        count
    }

    /// Check if we've seen a comm message with the given method.
    /// Marks matching messages as consumed.
    pub fn has_comm_method(&mut self, method: &str) -> bool {
        self.consume(|m| match m {
            Message::CommMsg(comm) => {
                comm.content.data.get("method").and_then(|v| v.as_str()) == Some(method)
            },
            _ => false,
        }) > 0
    }

    /// Check if we've seen at least N comm messages with the given method.
    /// Marks matching messages as consumed.
    pub fn has_comm_method_count(&mut self, method: &str, count: usize) -> bool {
        self.consume(|m| match m {
            Message::CommMsg(comm) => {
                comm.content.data.get("method").and_then(|v| v.as_str()) == Some(method)
            },
            _ => false,
        }) >= count
    }

    /// Check that predicates match messages in the given order.
    ///
    /// Each predicate must match a message at a strictly higher index than the
    /// previous predicate's match. This verifies synchronous message ordering
    /// (e.g., `ExecuteResult` before `Idle`).
    ///
    /// Marks matching messages as consumed.
    ///
    /// # Example
    ///
    /// ```ignore
    /// acc.in_order(&[
    ///     is_execute_result(),
    ///     is_idle(),
    /// ])
    /// ```
    pub fn in_order(&mut self, predicates: &[Box<dyn Fn(&Message) -> bool>]) -> bool {
        let mut last_idx: Option<usize> = None;

        for predicate in predicates {
            // Find the first matching message that comes after last_idx
            let found = self
                .messages
                .iter()
                .enumerate()
                .filter(|(i, _)| last_idx.map_or(true, |last| *i > last))
                .find(|(_, msg)| predicate(msg));

            match found {
                Some((idx, _)) => {
                    self.consumed.insert(idx);
                    last_idx = Some(idx);
                },
                None => return false,
            }
        }

        true
    }

    /// Check if we've seen an idle status message.
    /// Marks the idle status message as consumed.
    pub fn saw_idle(&mut self) -> bool {
        self.consume(|m| {
            matches!(
                m,
                Message::Status(s) if s.content.execution_state == ExecutionState::Idle
            )
        });
        self.saw_idle
    }
}

impl Default for MessageAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for MessageAccumulator {
    fn drop(&mut self) {
        if !self.verified {
            return;
        }
        if std::thread::panicking() {
            return;
        }

        let unconsumed: Vec<_> = self
            .messages
            .iter()
            .enumerate()
            .filter(|(i, msg)| {
                // Stream messages are exempt due to batching nondeterminism
                !matches!(msg, Message::Stream(_)) && !self.consumed.contains(i)
            })
            .collect();

        if !unconsumed.is_empty() {
            let descriptions: Vec<_> = unconsumed
                .iter()
                .map(|(i, msg)| format!("  [{i}] {msg:?}"))
                .collect();

            panic!(
                "MessageAccumulator dropped with {} unconsumed non-Stream message(s):\n{}\n\n\
                 This usually means the test condition didn't account for all messages.\n\
                 Either add checks for these messages or verify they're expected.",
                unconsumed.len(),
                descriptions.join("\n")
            );
        }
    }
}
