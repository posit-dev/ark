//
// iopub.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

//! Helper predicates for matching IOPub messages in tests.
//!
//! These functions return boxed predicates for use with `recv_iopub_async()`.

use amalthea::wire::jupyter_message::Message;
use amalthea::wire::status::ExecutionState;

type Predicate = Box<dyn FnMut(&Message) -> bool>;

/// Matches a `start_debug` comm message.
pub fn is_start_debug() -> Predicate {
    Box::new(|msg| {
        matches!(
            msg,
            Message::CommMsg(comm) if comm.content.data["method"] == "start_debug"
        )
    })
}

/// Matches a `stop_debug` comm message.
pub fn is_stop_debug() -> Predicate {
    Box::new(|msg| {
        matches!(
            msg,
            Message::CommMsg(comm) if comm.content.data["method"] == "stop_debug"
        )
    })
}

/// Matches an `Idle` status message.
pub fn is_idle() -> Predicate {
    Box::new(|msg| {
        matches!(
            msg,
            Message::Status(s) if s.content.execution_state == ExecutionState::Idle
        )
    })
}

/// Matches a Stream message containing the given text.
pub fn stream_contains(text: &'static str) -> Predicate {
    Box::new(move |msg| {
        let Message::Stream(stream) = msg else {
            return false;
        };
        stream.content.text.contains(text)
    })
}

/// Matches a Stream message containing all of the given texts in order.
pub fn stream_contains_all(texts: &'static [&'static str]) -> Predicate {
    Box::new(move |msg| {
        let Message::Stream(stream) = msg else {
            return false;
        };
        let content = &stream.content.text;
        let mut pos = 0;
        for text in texts {
            match content[pos..].find(text) {
                Some(found) => pos += found + text.len(),
                None => return false,
            }
        }
        true
    })
}

/// Matches an ExecuteResult message.
pub fn is_execute_result() -> Predicate {
    Box::new(|msg| matches!(msg, Message::ExecuteResult(_)))
}

/// Matches an ExecuteResult message containing the given text.
pub fn execute_result_contains(text: &'static str) -> Predicate {
    Box::new(move |msg| {
        let Message::ExecuteResult(result) = msg else {
            return false;
        };
        let content = result.content.data["text/plain"].as_str().unwrap_or("");
        content.contains(text)
    })
}

/// Matches any Stream message.
pub fn is_stream() -> Predicate {
    Box::new(|msg| matches!(msg, Message::Stream(_)))
}
