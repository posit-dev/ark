use std::cell::Cell;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::Seek;
use std::io::Write;
use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::time::Duration;
use std::time::Instant;

use amalthea::comm::data_explorer_comm::DataExplorerFrontendEvent;
use amalthea::comm::variables_comm::RefreshParams;
use amalthea::comm::variables_comm::UpdateParams;
use amalthea::comm::variables_comm::VariablesFrontendEvent;
use amalthea::fixtures::dummy_frontend::DummyConnection;
use amalthea::fixtures::dummy_frontend::DummyFrontend;
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::comm_close::CommClose;
use amalthea::wire::comm_open::CommOpen;
use amalthea::wire::execute_request::ExecuteRequestPositron;
use amalthea::wire::execute_request::JupyterPositronLocation;
use amalthea::wire::execute_request::JupyterPositronPosition;
use amalthea::wire::execute_request::JupyterPositronRange;
use amalthea::wire::jupyter_message::Message;
use amalthea::wire::stream::Stream;
use ark::console::SessionMode;
use ark::repos::DefaultRepos;
use ark::url::ExtUrl;
use regex::Regex;
use tempfile::NamedTempFile;

use crate::comm::RECV_TIMEOUT;
use crate::tracing::trace_iopub_msg;
use crate::tracing::trace_separator;
use crate::tracing::trace_shell_reply;
use crate::tracing::trace_shell_request;
use crate::DapClient;
use crate::LspClient;

// There can be only one frontend per process. Needs to be in a mutex because
// the frontend wraps zmq sockets which are unsafe to send across threads.
//
// This is using `OnceLock` because it provides a way of checking whether the
// value has been initialized already. Also we'll need to parameterize
// initialization in the future.
static FRONTEND: OnceLock<Arc<Mutex<DummyFrontend>>> = OnceLock::new();

/// Wrapper around `DummyFrontend` that checks sockets are empty on drop.
///
/// This wrapper automatically buffers Stream messages when receiving from IOPub,
/// allowing tests to assert on streams separately from the main message flow.
/// This handles the non-deterministic interleaving of streams (due to arbitrary
/// batching and splitting of messages by R) with other messages.
pub struct DummyArkFrontend {
    guard: MutexGuard<'static, DummyFrontend>,
    /// Accumulated stdout stream content
    stream_stdout: RefCell<String>,
    /// Accumulated stderr stream content
    stream_stderr: RefCell<String>,
    /// Put-back queue for non-stream IOPub messages encountered during stream assertions
    pending_iopub_messages: RefCell<VecDeque<Message>>,
    /// Tracks whether any stream assertion was made (for Drop validation).
    /// If stream is emitted during a test, there must be at least one stream
    /// assertion.
    streams_handled: Cell<bool>,
    /// Whether we're currently in a debug context (between start_debug and stop_debug)
    in_debug: Cell<bool>,
    /// Comm ID of the open variables comm, if any.
    variables_comm_id: RefCell<Option<String>>,
    /// Buffered variables comm events, auto-collected by `recv_iopub_next()`.
    /// Buffering is needed because variables events can race with Idle on
    /// IOPub. Once https://github.com/posit-dev/ark/issues/689 is resolved,
    /// we should be able to assert these deterministically in the message
    /// sequence instead.
    variables_events: RefCell<VecDeque<VariablesFrontendEvent>>,
    /// Auto-buffered data explorer state.
    data_explorer: DataExplorerBuffer,
}

/// A buffered data explorer message, preserving arrival order across event
/// types so tests can assert on sequencing.
#[derive(Debug)]
enum DataExplorerMessage {
    Event(DataExplorerFrontendEvent),
    Close(String),
}

/// Buffers data explorer comm messages that arrive asynchronously on IOPub.
///
/// The data explorer spawns a background thread that sends CommOpen, CommMsg
/// (events), and CommClose independently of the execute request lifecycle.
/// These can race with Idle and other messages, so we buffer them here and
/// provide methods to consume them in tests.
struct DataExplorerBuffer {
    /// Comm IDs of open data explorer comms.
    comm_ids: RefCell<Vec<String>>,
    /// Buffered messages in arrival order.
    messages: RefCell<VecDeque<DataExplorerMessage>>,
}

impl DataExplorerBuffer {
    fn new() -> Self {
        Self {
            comm_ids: RefCell::new(Vec::new()),
            messages: RefCell::new(VecDeque::new()),
        }
    }

    fn is_data_explorer_comm(&self, comm_id: &str) -> bool {
        self.comm_ids.borrow().iter().any(|id| id == comm_id)
    }

    fn track_open(&self, comm_id: String) {
        self.comm_ids.borrow_mut().push(comm_id);
    }

    fn buffer_event(&self, data: &serde_json::Value) {
        let event: DataExplorerFrontendEvent = serde_json::from_value(data.clone()).unwrap();
        self.messages
            .borrow_mut()
            .push_back(DataExplorerMessage::Event(event));
    }

    fn buffer_close(&self, close: &CommClose) {
        self.comm_ids.borrow_mut().retain(|id| id != &close.comm_id);
        self.messages
            .borrow_mut()
            .push_back(DataExplorerMessage::Close(close.comm_id.clone()));
    }

    /// Pop the next message if it's an `Event`.
    /// Returns `None` if the buffer is empty.
    /// Panics if the next message is a `Close` (wrong receive method).
    fn pop_event(&self) -> Option<DataExplorerFrontendEvent> {
        let mut messages = self.messages.borrow_mut();
        match messages.front() {
            Some(DataExplorerMessage::Event(_)) => match messages.pop_front() {
                Some(DataExplorerMessage::Event(event)) => Some(event),
                _ => None,
            },
            Some(other) => panic!("Expected data explorer Event, got {other:?}"),
            None => None,
        }
    }

    /// Pop the next message if it's a `Close`.
    /// Returns `None` if the buffer is empty.
    /// Panics if the next message is an `Event` (wrong receive method).
    fn pop_close(&self) -> Option<String> {
        let mut messages = self.messages.borrow_mut();
        match messages.front() {
            Some(DataExplorerMessage::Close(_)) => match messages.pop_front() {
                Some(DataExplorerMessage::Close(id)) => Some(id),
                _ => None,
            },
            Some(other) => panic!("Expected data explorer Close, got {other:?}"),
            None => None,
        }
    }

    fn is_empty(&self) -> bool {
        self.messages.borrow().is_empty()
    }

    /// Panic if any messages were buffered but never consumed.
    fn assert_consumed(&self) {
        let messages = self.messages.borrow();
        if !messages.is_empty() {
            panic!(
                "Test has {} unconsumed data explorer message(s): {:?}",
                messages.len(),
                *messages
            );
        }
    }
}

/// Result of draining accumulated streams
pub struct DrainedStreams {
    pub stdout: String,
    pub stderr: String,
}

/// CI-aware timeout for draining streams.
/// Shorter locally for fast iteration, longer on CI where things are slower.
fn default_drain_timeout() -> Duration {
    if std::env::var("CI").is_ok() {
        Duration::from_millis(200)
    } else {
        Duration::from_millis(50)
    }
}

struct DummyArkFrontendOptions {
    interactive: bool,
    site_r_profile: bool,
    user_r_profile: bool,
    r_environ: bool,
    session_mode: SessionMode,
    default_repos: DefaultRepos,
    startup_file: Option<String>,
}

/// Wrapper around `DummyArkFrontend` that uses `SessionMode::Notebook`
///
/// Only one of `DummyArkFrontend` or `DummyArkFrontendNotebook` can be used in
/// a given process. Just don't import both and you should be fine as Rust will
/// let you know about a missing symbol if you happen to copy paste `lock()`
/// calls of different kernel types between files.
pub struct DummyArkFrontendNotebook {
    inner: DummyArkFrontend,
}

/// Wrapper around `DummyArkFrontend` that allows an `.Rprofile` to run
pub struct DummyArkFrontendRprofile {
    inner: DummyArkFrontend,
}

/// Wrapper around `DummyArkFrontend` that allows setting default repos
/// for the frontend
pub struct DummyArkFrontendDefaultRepos {
    inner: DummyArkFrontend,
}

impl DummyArkFrontend {
    pub fn lock() -> Self {
        Self {
            guard: Self::get_frontend().lock().unwrap(),
            stream_stdout: RefCell::new(String::new()),
            stream_stderr: RefCell::new(String::new()),
            pending_iopub_messages: RefCell::new(VecDeque::new()),
            streams_handled: Cell::new(false),
            in_debug: Cell::new(false),
            variables_comm_id: RefCell::new(None),
            variables_events: RefCell::new(VecDeque::new()),
            data_explorer: DataExplorerBuffer::new(),
        }
    }

    /// Buffer a stream message into the appropriate accumulator.
    fn buffer_stream(&self, data: &amalthea::wire::stream::StreamOutput) {
        match data.name {
            Stream::Stdout => self.stream_stdout.borrow_mut().push_str(&data.text),
            Stream::Stderr => self.stream_stderr.borrow_mut().push_str(&data.text),
        }
    }

    /// Receive from IOPub with a timeout.
    /// Returns `None` if the timeout expires before a message arrives.
    #[cfg(not(all(target_os = "windows", target_arch = "aarch64")))]
    fn recv_iopub_with_timeout(&self, timeout: Duration) -> Option<Message> {
        let timeout_ms = timeout.as_millis() as i64;
        if self.guard.iopub_socket.poll_incoming(timeout_ms).unwrap() {
            Some(Message::read_from_socket(&self.guard.iopub_socket).unwrap())
        } else {
            None
        }
    }

    /// Receive from IOPub with a timeout.
    /// Returns `None` if the timeout expires before a message arrives.
    ///
    /// On Windows ARM, ZMQ poll with timeout blocks forever instead of
    /// respecting the timeout. Use non-blocking poll with manual timing.
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    fn recv_iopub_with_timeout(&self, timeout: Duration) -> Option<Message> {
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() >= timeout {
                return None;
            }

            // Use non-blocking poll (timeout=0) to avoid ZMQ blocking forever
            match self.guard.iopub_socket.poll_incoming(0) {
                Ok(true) => {
                    return Some(Message::read_from_socket(&self.guard.iopub_socket).unwrap());
                },
                Ok(false) => {
                    // No message available, sleep briefly and try again
                    std::thread::sleep(Duration::from_millis(10));
                },
                Err(_) => return None,
            }
        }
    }

    /// Core primitive: receive the next non-stream, non-variables-comm message
    /// from IOPub.
    ///
    /// This automatically buffers any Stream messages and variables comm
    /// messages encountered while waiting. All `recv_iopub_*` methods for
    /// non-stream messages should use this instead of calling `recv_iopub()`
    /// directly.
    fn recv_iopub_next(&self) -> Message {
        // Check put-back buffer first
        if let Some(msg) = self.pending_iopub_messages.borrow_mut().pop_front() {
            trace_iopub_msg(&msg);
            return msg;
        }
        // Read from socket, auto-buffering streams and comm messages
        loop {
            let msg = self.recv_iopub();
            if !self.try_buffer_msg(&msg) {
                trace_iopub_msg(&msg);
                return msg;
            }
        }
    }

    /// Try to buffer a known message (stream, variables comm, or data explorer comm).
    /// Traces the message if it was buffered. Returns `true` if the message was consumed.
    ///
    /// Comm message buffering is needed because comm events can race with Idle
    /// on IOPub. Once https://github.com/posit-dev/ark/issues/689 is resolved,
    /// comm events should arrive deterministically in the message sequence and
    /// this buffering can be removed.
    fn try_buffer_msg(&self, msg: &Message) -> bool {
        match msg {
            Message::Stream(ref data) => {
                trace_iopub_msg(msg);
                self.buffer_stream(&data.content);
                true
            },
            Message::CommMsg(ref data) if self.is_variables_comm(&data.content.comm_id) => {
                trace_iopub_msg(msg);
                self.buffer_variables_event(&data.content.data);
                true
            },
            Message::CommMsg(ref data)
                if self
                    .data_explorer
                    .is_data_explorer_comm(&data.content.comm_id) =>
            {
                trace_iopub_msg(msg);
                self.data_explorer.buffer_event(&data.content.data);
                true
            },
            Message::CommClose(ref data)
                if self
                    .data_explorer
                    .is_data_explorer_comm(&data.content.comm_id) =>
            {
                trace_iopub_msg(msg);
                self.data_explorer.buffer_close(&data.content);
                true
            },
            _ => false,
        }
    }

    fn is_variables_comm(&self, comm_id: &str) -> bool {
        self.variables_comm_id
            .borrow()
            .as_deref()
            .is_some_and(|id| id == comm_id)
    }

    fn buffer_variables_event(&self, data: &serde_json::Value) {
        let event: VariablesFrontendEvent = serde_json::from_value(data.clone()).unwrap();
        self.variables_events.borrow_mut().push_back(event);
    }

    /// Internal helper for stream assertions.
    #[track_caller]
    fn assert_stream_contains(&self, buffer: &RefCell<String>, stream_name: &str, expected: &str) {
        self.streams_handled.set(true);
        let deadline = Instant::now() + RECV_TIMEOUT;

        loop {
            // Check buffer
            {
                let mut buf = buffer.borrow_mut();
                if let Some(pos) = buf.find(expected) {
                    // Found it! Drain buffer up to the expected string
                    buf.drain(..pos + expected.len());
                    return;
                }
            }

            // Timeout check
            if Instant::now() >= deadline {
                panic!(
                    "Timeout waiting for {stream_name} containing {expected:?}\n\
                     Accumulated stdout: {:?}\n\
                     Accumulated stderr: {:?}",
                    self.stream_stdout.borrow(),
                    self.stream_stderr.borrow()
                );
            }

            // Read more (with short timeout to allow checking deadline)
            let remaining = deadline.saturating_duration_since(Instant::now());
            let poll_timeout = remaining.min(Duration::from_millis(100));

            if let Some(msg) = self.recv_iopub_with_timeout(poll_timeout) {
                if !self.try_buffer_msg(&msg) {
                    self.pending_iopub_messages.borrow_mut().push_back(msg);
                }
            }
        }
    }

    /// Assert that stdout contains the expected text.
    ///
    /// This checks the accumulated stream buffer first. If the expected text
    /// isn't found, it reads more messages from IOPub (buffering any non-stream
    /// messages for later) until the text is found or a timeout occurs.
    ///
    /// The buffer is drained up to and including the match point; any content
    /// after the match remains for future assertions. This means assertions are
    /// order-sensitive: assert in the order you expect the text to appear.
    #[track_caller]
    pub fn assert_stream_stdout_contains(&self, expected: &str) {
        self.assert_stream_contains(&self.stream_stdout, "stdout", expected);
    }

    /// Assert that stderr contains the expected text.
    ///
    /// This checks the accumulated stream buffer first. If the expected text
    /// isn't found, it reads more messages from IOPub (buffering any non-stream
    /// messages for later) until the text is found or a timeout occurs.
    ///
    /// The buffer is drained up to and including the match point; any content
    /// after the match remains for future assertions. This means assertions are
    /// order-sensitive: assert in the order you expect the text to appear.
    #[track_caller]
    pub fn assert_stream_stderr_contains(&self, expected: &str) {
        self.assert_stream_contains(&self.stream_stderr, "stderr", expected);
    }

    /// Assert that stdout matches the given regex pattern.
    ///
    /// Like `assert_stream_stdout_contains`, but uses regex matching.
    /// The buffer is drained up to and including the match.
    #[track_caller]
    pub fn assert_stream_stdout_matches(&self, pattern: &Regex) {
        self.assert_stream_matches_re(&self.stream_stdout, "stdout", pattern);
    }

    /// Assert that stderr matches the given regex pattern.
    ///
    /// Like `assert_stream_stderr_contains`, but uses regex matching.
    /// The buffer is drained up to and including the match.
    #[track_caller]
    pub fn assert_stream_stderr_matches(&self, pattern: &Regex) {
        self.assert_stream_matches_re(&self.stream_stderr, "stderr", pattern);
    }

    /// Assert that stdout contains a "debug at" message referencing the given file.
    ///
    /// R outputs "debug at <path>#<line>: <code>" when stepping through sourced files.
    /// This helper checks both "debug at" and the filename appear in stdout.
    #[track_caller]
    pub fn assert_stream_debug_at(&self, file: &SourceFile) {
        self.assert_stream_stdout_contains("debug at");
        self.assert_stream_stdout_contains(&file.filename);
    }

    /// Internal helper for regex stream assertions.
    #[track_caller]
    fn assert_stream_matches_re(
        &self,
        buffer: &RefCell<String>,
        stream_name: &str,
        pattern: &Regex,
    ) {
        self.streams_handled.set(true);
        let deadline = Instant::now() + RECV_TIMEOUT;

        loop {
            // Check buffer
            {
                let mut buf = buffer.borrow_mut();
                if let Some(m) = pattern.find(&buf) {
                    // Found it! Drain buffer up to the end of the match
                    let end = m.end();
                    buf.drain(..end);
                    return;
                }
            }

            // Timeout check
            if Instant::now() >= deadline {
                panic!(
                    "Timeout waiting for {stream_name} matching {pattern:?}\n\
                     Accumulated stdout: {:?}\n\
                     Accumulated stderr: {:?}",
                    self.stream_stdout.borrow(),
                    self.stream_stderr.borrow()
                );
            }

            // Read more (with short timeout to allow checking deadline)
            let remaining = deadline.saturating_duration_since(Instant::now());
            let poll_timeout = remaining.min(Duration::from_millis(100));

            if let Some(msg) = self.recv_iopub_with_timeout(poll_timeout) {
                if !self.try_buffer_msg(&msg) {
                    self.pending_iopub_messages.borrow_mut().push_back(msg);
                }
            }
        }
    }

    /// Drain accumulated streams, waiting for any stragglers.
    ///
    /// Normally, streams are asserted with `assert_stream_*_contains()` and
    /// flushed automatically at idle boundaries via `recv_iopub_idle()`.
    /// Use this method only for edge cases like:
    /// - Streams that may arrive during another operation's idle boundary (race conditions)
    /// - Ordering assertions where you need to capture content at a specific point
    ///
    /// Returns the accumulated stdout and stderr content, clearing the buffers.
    pub fn drain_streams(&self) -> DrainedStreams {
        self.streams_handled.set(true);
        self.drain_streams_internal()
    }

    /// Internal drain that doesn't set `streams_handled` (for use in Drop).
    fn drain_streams_internal(&self) -> DrainedStreams {
        let deadline = Instant::now() + default_drain_timeout();

        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.recv_iopub_with_timeout(remaining) {
                Some(msg) => match &msg {
                    Message::Stream(data) => {
                        trace_iopub_msg(&msg);
                        self.buffer_stream(&data.content);
                    },
                    _ => {
                        self.pending_iopub_messages.borrow_mut().push_back(msg);
                        break;
                    },
                },
                None => break,
            }
        }

        DrainedStreams {
            stdout: std::mem::take(&mut self.stream_stdout.borrow_mut()),
            stderr: std::mem::take(&mut self.stream_stderr.borrow_mut()),
        }
    }

    // Shadow DummyFrontend's stream methods to prevent bypassing the buffering layer.
    // These methods read directly from the IOPub socket, which breaks the stream
    // accumulation invariant. Use `assert_stream_*_contains()` instead.

    #[allow(unused)]
    pub fn recv_iopub_stream_stdout(&self, _expect: &str) {
        panic!("Use assert_stream_stdout_contains() instead of recv_iopub_stream_stdout()");
    }

    #[allow(unused)]
    pub fn recv_iopub_stream_stderr(&self, _expect: &str) {
        panic!("Use assert_stream_stderr_contains() instead of recv_iopub_stream_stderr()");
    }

    #[allow(unused)]
    pub fn recv_iopub_stream_stdout_with<F>(&self, _f: F)
    where
        F: FnMut(&str),
    {
        panic!(
            "Use assert_stream_stdout_contains() or assert_stream_stdout_matches() \
             instead of recv_iopub_stream_stdout_with()"
        );
    }

    #[allow(unused)]
    pub fn recv_iopub_stream_stderr_with<F>(&self, _f: F)
    where
        F: FnMut(&str),
    {
        panic!(
            "Use assert_stream_stderr_contains() or assert_stream_stderr_matches() \
             instead of recv_iopub_stream_stderr_with()"
        );
    }

    // Overriding methods for base DummyFrontend's `recv_iopub_` methods. These
    // use `recv_iopub_next()` to auto-skip streams. Question: Maybe this
    // behaviour should live in the base dummy frontend? The stream issues are
    // likely not R specific.

    /// Receive from IOPub and assert Busy status.
    /// Automatically skips any Stream messages.
    #[track_caller]
    pub fn recv_iopub_busy(&self) {
        let msg = self.recv_iopub_next();
        match msg {
            Message::Status(data) => {
                assert_eq!(
                    data.content.execution_state,
                    amalthea::wire::status::ExecutionState::Busy,
                    "Expected Busy status"
                );
            },
            other => panic!("Expected Busy status, got {:?}", other),
        }
    }

    /// Receive from IOPub and assert Idle status.
    /// Automatically skips any Stream messages.
    ///
    /// This method acts as a synchronization point: after receiving Idle,
    /// it flushes stream buffers and panics if streams were received but not
    /// asserted. This enforces that stream assertions are made within their
    /// busy/idle window.
    #[track_caller]
    pub fn recv_iopub_idle(&self) {
        self.recv_iopub_idle_impl();
        self.flush_streams_at_boundary();
    }

    /// Receive Idle and return accumulated streams.
    ///
    /// Use this when you need to collect stream output from an execution rather
    /// than asserting on it. This is the "collect" counterpart to the "assert"
    /// pattern used by `recv_iopub_idle()` + `assert_stream_*_contains()`.
    #[track_caller]
    pub fn recv_iopub_idle_and_flush(&self) -> DrainedStreams {
        self.recv_iopub_idle_impl();
        self.streams_handled.set(true);
        self.drain_streams_internal()
    }

    #[track_caller]
    fn recv_iopub_idle_impl(&self) {
        let msg = self.recv_iopub_next();
        match msg {
            Message::Status(data) => {
                assert_eq!(
                    data.content.execution_state,
                    amalthea::wire::status::ExecutionState::Idle,
                    "Expected Idle status"
                );
            },
            other => panic!("Expected Idle status, got {:?}", other),
        }
    }

    /// Flush stream buffers at an idle boundary.
    ///
    /// Panics if streams were received but not asserted since the last boundary.
    /// After flushing, resets the `streams_handled` flag for the next busy/idle cycle.
    #[track_caller]
    fn flush_streams_at_boundary(&self) {
        let has_streams =
            !self.stream_stdout.borrow().is_empty() || !self.stream_stderr.borrow().is_empty();

        if has_streams && !self.streams_handled.get() {
            panic!(
                "Streams were received but not asserted before idle boundary.\n\
                 stdout: {:?}\n\
                 stderr: {:?}",
                self.stream_stdout.borrow(),
                self.stream_stderr.borrow()
            );
        }

        // Clear buffers and reset flag for next operation
        self.stream_stdout.borrow_mut().clear();
        self.stream_stderr.borrow_mut().clear();
        self.streams_handled.set(false);
    }

    /// Receive from IOPub and assert ExecuteInput message.
    /// Automatically skips any Stream messages.
    #[track_caller]
    pub fn recv_iopub_execute_input(&self) -> amalthea::wire::execute_input::ExecuteInput {
        let msg = self.recv_iopub_next();
        match msg {
            Message::ExecuteInput(data) => data.content,
            other => panic!("Expected ExecuteInput, got {:?}", other),
        }
    }

    /// Receive from IOPub and assert ExecuteResult message.
    /// Automatically skips any Stream messages.
    /// Returns the `text/plain` result.
    #[track_caller]
    pub fn recv_iopub_execute_result(&self) -> String {
        let msg = self.recv_iopub_next();
        match msg {
            Message::ExecuteResult(data) => match data.content.data {
                serde_json::Value::Object(map) => match &map["text/plain"] {
                    serde_json::Value::String(s) => s.clone(),
                    other => panic!("Expected text/plain to be String, got {:?}", other),
                },
                other => panic!("Expected ExecuteResult data to be Object, got {:?}", other),
            },
            other => panic!("Expected ExecuteResult, got {:?}", other),
        }
    }

    /// Receive from IOPub and assert ExecuteError message.
    /// Automatically skips any Stream messages.
    /// Returns the `evalue` field.
    #[track_caller]
    pub fn recv_iopub_execute_error(&self) -> String {
        let msg = self.recv_iopub_next();
        match msg {
            Message::ExecuteError(data) => data.content.exception.evalue,
            other => panic!("Expected ExecuteError, got {:?}", other),
        }
    }

    /// Receive from IOPub and assert DisplayData message.
    /// Automatically skips any Stream messages.
    #[track_caller]
    pub fn recv_iopub_display_data(&self) {
        let msg = self.recv_iopub_next();
        match msg {
            Message::DisplayData(_) => {},
            other => panic!("Expected DisplayData, got {:?}", other),
        }
    }

    /// Receive from IOPub and assert CommMsg message.
    /// Automatically skips any Stream messages.
    #[track_caller]
    pub fn recv_iopub_comm_msg(&self) -> amalthea::wire::comm_msg::CommWireMsg {
        let msg = self.recv_iopub_next();
        match msg {
            Message::CommMsg(data) => data.content,
            other => panic!("Expected CommMsg, got {:?}", other),
        }
    }

    /// Receive from IOPub and assert CommOpen message.
    /// Automatically skips any Stream messages.
    #[track_caller]
    pub fn recv_iopub_comm_open(&self) -> amalthea::wire::comm_open::CommOpen {
        let msg = self.recv_iopub_next();
        match msg {
            Message::CommOpen(data) => data.content,
            other => panic!("Expected CommOpen, got {:?}", other),
        }
    }

    /// Receive from IOPub and assert CommClose message.
    /// Automatically skips any Stream messages.
    /// Returns the comm_id.
    #[track_caller]
    pub fn recv_iopub_comm_close(&self) -> String {
        let msg = self.recv_iopub_next();
        match msg {
            Message::CommClose(data) => data.content.comm_id,
            other => panic!("Expected CommClose, got {:?}", other),
        }
    }

    /// Wait for R cleanup to start (indicating shutdown has been initiated).
    /// Panics if cleanup doesn't start within the timeout.
    #[cfg(unix)]
    #[track_caller]
    pub fn wait_for_cleanup() {
        use std::time::Duration;

        use ark::sys::console::CLEANUP_SIGNAL;

        let (lock, cvar) = &CLEANUP_SIGNAL;
        let result = cvar
            .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(3), |started| {
                !*started
            })
            .unwrap();

        if !*result.0 {
            panic!("Cleanup did not start within timeout");
        }
    }

    /// Start DAP server via comm protocol and return a connected client.
    ///
    /// This sends a `comm_open` message to start the DAP server, waits for
    /// the `server_started` response with the port, and connects a `DapClient`.
    #[track_caller]
    pub fn start_dap(&self) -> DapClient {
        let port = self.start_server("ark_dap");
        let mut client = DapClient::connect("127.0.0.1", port).unwrap();
        client.initialize();
        client.attach();
        client
    }

    /// Start LSP server via comm protocol and return a connected client.
    ///
    /// This sends a `comm_open` message to start the LSP server, waits for
    /// the `server_started` response with the port, and connects an `LspClient`.
    #[track_caller]
    pub fn start_lsp(&self) -> LspClient {
        let port = self.start_server("lsp");
        let mut client = LspClient::connect("127.0.0.1", port).unwrap();
        client.initialize();
        client
    }

    /// Open a server comm and wait for the `server_started` message with the port.
    #[track_caller]
    fn start_server(&self, target_name: &str) -> u16 {
        let comm_id = uuid::Uuid::new_v4().to_string();

        self.send_shell(CommOpen {
            comm_id: comm_id.clone(),
            target_name: String::from(target_name),
            data: serde_json::json!({ "ip_address": "127.0.0.1" }),
        });

        self.recv_iopub_busy();

        let comm_msg = self.recv_iopub_comm_msg();
        assert_eq!(comm_msg.comm_id, comm_id);

        let method = comm_msg.data["method"]
            .as_str()
            .expect("Expected method field");
        assert_eq!(method, "server_started");

        let port = comm_msg.data["params"]["port"]
            .as_u64()
            .expect("Expected port field") as u16;

        self.recv_iopub_idle();

        port
    }

    /// Open a `positron.variables` comm via the Shell socket.
    ///
    /// Returns the initial `RefreshParams` from the variables pane.
    /// After this call, variables comm messages are automatically buffered
    /// by `recv_iopub_next()` (parallel to how Stream messages are handled).
    #[track_caller]
    pub fn open_variables_comm(&self) -> RefreshParams {
        debug_assert!(
            self.variables_comm_id.borrow().is_none(),
            "Variables comm already open"
        );

        let comm_id = uuid::Uuid::new_v4().to_string();
        *self.variables_comm_id.borrow_mut() = Some(comm_id.clone());

        self.send_shell(CommOpen {
            comm_id: comm_id.clone(),
            target_name: String::from("positron.variables"),
            data: serde_json::json!({}),
        });

        // Busy + Idle from the comm_open handler. The initial Refresh from
        // the variables thread may arrive interleaved or shortly after;
        // `recv_iopub_next()` auto-buffers it.
        self.recv_iopub_busy();
        self.recv_iopub_idle();

        // The initial Refresh may already be buffered, or we need to wait.
        self.recv_variables_refresh()
    }

    /// Wait for the next variables `Refresh` event.
    ///
    /// Checks the internal buffer first (populated by `recv_iopub_next()`),
    /// then reads more IOPub messages if needed.
    #[track_caller]
    pub fn recv_variables_refresh(&self) -> RefreshParams {
        match self.recv_variables_event() {
            VariablesFrontendEvent::Refresh(params) => params,
            other => panic!("Expected variables Refresh, got {other:?}"),
        }
    }

    /// Wait for the next variables `Update` event.
    ///
    /// Checks the internal buffer first (populated by `recv_iopub_next()`),
    /// then reads more IOPub messages if needed.
    #[track_caller]
    pub fn recv_variables_update(&self) -> UpdateParams {
        match self.recv_variables_event() {
            VariablesFrontendEvent::Update(params) => params,
            other => panic!("Expected variables Update, got {other:?}"),
        }
    }

    /// Wait for the next variables comm event (Refresh or Update).
    /// This polling loop exists because variables events can race with Idle
    /// on IOPub. Once https://github.com/posit-dev/ark/issues/689 is
    /// resolved, this can be replaced by a direct `recv_iopub_next()` call.
    #[track_caller]
    fn recv_variables_event(&self) -> VariablesFrontendEvent {
        // Check buffer first
        if let Some(event) = self.variables_events.borrow_mut().pop_front() {
            return event;
        }

        // Read more IOPub messages until we get a variables event
        let deadline = Instant::now() + RECV_TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                panic!("Timeout waiting for variables comm event");
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            let poll_timeout = remaining.min(Duration::from_millis(100));

            if let Some(msg) = self.recv_iopub_with_timeout(poll_timeout) {
                if self.try_buffer_msg(&msg) {
                    if let Some(event) = self.variables_events.borrow_mut().pop_front() {
                        return event;
                    }
                    continue;
                }
                // Idle can race with variables events (see #689)
                match msg {
                    Message::Status(_) => {
                        self.pending_iopub_messages.borrow_mut().push_back(msg);
                    },
                    other => {
                        panic!("Unexpected message while waiting for variables event: {other:?}")
                    },
                }
            }
        }
    }

    /// Execute `View(var_name)` and track the resulting data explorer comm.
    ///
    /// Returns the comm ID. After this call, data explorer comm messages are
    /// automatically buffered by `recv_iopub_next()` (parallel to how Stream
    /// and Variables messages are handled).
    ///
    /// Note: The data explorer comm is opened asynchronously by a spawned thread,
    /// so the CommOpen message may arrive after the execute request completes.
    #[track_caller]
    pub fn open_data_explorer(&self, var_name: &str) -> String {
        self.send_execute_request(
            &format!("View({var_name})"),
            ExecuteRequestOptions::default(),
        );
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_idle();
        self.recv_shell_execute_reply();

        // The CommOpen is sent asynchronously by the data explorer thread,
        // so we need to wait for it separately after the execute completes.
        let comm_open = self.recv_iopub_comm_open();
        assert_eq!(
            comm_open.target_name, "positron.dataExplorer",
            "Expected data explorer comm, got {:?}",
            comm_open.target_name
        );

        let comm_id = comm_open.comm_id;
        self.data_explorer.track_open(comm_id.clone());

        comm_id
    }

    /// Receive the next data explorer event from the buffer.
    ///
    /// Checks the internal buffer first (populated by `recv_iopub_next()`),
    /// then reads more IOPub messages if needed, with timeout.
    #[track_caller]
    pub fn recv_data_explorer_event(&self) -> DataExplorerFrontendEvent {
        // Check buffer first
        if let Some(event) = self.data_explorer.pop_event() {
            return event;
        }

        // Read more IOPub messages until we get a data explorer event
        let deadline = Instant::now() + RECV_TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                panic!("Timeout waiting for data explorer event");
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            let poll_timeout = remaining.min(Duration::from_millis(100));

            if let Some(msg) = self.recv_iopub_with_timeout(poll_timeout) {
                if self.try_buffer_msg(&msg) {
                    if let Some(event) = self.data_explorer.pop_event() {
                        return event;
                    }
                    continue;
                }
                match msg {
                    Message::Status(_) => {
                        self.pending_iopub_messages.borrow_mut().push_back(msg);
                    },
                    other => {
                        panic!(
                            "Unexpected message while waiting for data explorer event: {other:?}"
                        )
                    },
                }
            }
        }
    }

    /// Receive the next data explorer CommClose from the buffer.
    ///
    /// Checks the internal buffer first (populated by `try_buffer_msg()`),
    /// then reads more IOPub messages if needed, with timeout.
    #[track_caller]
    pub fn recv_data_explorer_close(&self) -> String {
        if let Some(comm_id) = self.data_explorer.pop_close() {
            return comm_id;
        }

        let deadline = Instant::now() + RECV_TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                panic!("Timeout waiting for data explorer CommClose");
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            let poll_timeout = remaining.min(Duration::from_millis(100));

            if let Some(msg) = self.recv_iopub_with_timeout(poll_timeout) {
                if self.try_buffer_msg(&msg) {
                    if let Some(comm_id) = self.data_explorer.pop_close() {
                        return comm_id;
                    }
                    continue;
                }
                match msg {
                    Message::Status(_) => {
                        self.pending_iopub_messages.borrow_mut().push_back(msg);
                    },
                    other => {
                        panic!(
                            "Unexpected message while waiting for data explorer CommClose: {other:?}"
                        )
                    },
                }
            }
        }
    }

    /// Assert that no data explorer events are buffered.
    ///
    /// Drains IOPub briefly to catch any stragglers before checking.
    #[track_caller]
    pub fn assert_no_data_explorer_events(&self) {
        // Brief drain to catch any in-flight messages
        let deadline = Instant::now() + default_drain_timeout();

        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.recv_iopub_with_timeout(remaining) {
                Some(msg) => {
                    if !self.try_buffer_msg(&msg) {
                        self.pending_iopub_messages.borrow_mut().push_back(msg);
                        break;
                    }
                },
                None => break,
            }
        }

        if !self.data_explorer.is_empty() {
            self.data_explorer.assert_consumed();
        }
    }

    /// Receive from IOPub and assert a `start_debug` comm message.
    /// Automatically skips any Stream messages.
    #[track_caller]
    pub fn recv_iopub_start_debug(&self) {
        let msg = self.recv_iopub_next();
        match msg {
            Message::CommMsg(data) => {
                let method = data.content.data.get("method").and_then(|v| v.as_str());
                assert_eq!(
                    method,
                    Some("start_debug"),
                    "Expected start_debug comm message"
                );
            },
            other => panic!("Expected CommMsg with start_debug, got {:?}", other),
        }
        self.in_debug.set(true);
    }

    /// Receive from IOPub and assert a `stop_debug` comm message.
    /// Automatically skips any Stream messages.
    #[track_caller]
    pub fn recv_iopub_stop_debug(&self) {
        let msg = self.recv_iopub_next();
        match msg {
            Message::CommMsg(data) => {
                let method = data.content.data.get("method").and_then(|v| v.as_str());
                assert_eq!(
                    method,
                    Some("stop_debug"),
                    "Expected stop_debug comm message"
                );
            },
            other => panic!("Expected CommMsg with stop_debug, got {:?}", other),
        }
        self.in_debug.set(false);
    }

    /// Whether the frontend is currently in a debug context.
    pub fn in_debug(&self) -> bool {
        self.in_debug.get()
    }

    /// Sends an execute request and handles the standard message flow:
    /// busy -> execute_input -> idle -> execute_reply.
    /// Asserts that the input code matches and returns the execution count.
    #[track_caller]
    pub fn execute_request_invisibly(&self, code: &str) -> u32 {
        self.send_execute_request(code, ExecuteRequestOptions::default());
        self.recv_iopub_busy();

        let input = self.recv_iopub_execute_input();
        assert_eq!(input.code, code);

        self.recv_iopub_idle();

        let execution_count = self.recv_shell_execute_reply();
        assert_eq!(execution_count, input.execution_count);

        execution_count
    }

    /// Send an execute request with tracing
    #[track_caller]
    pub fn send_execute_request_traced(&self, code: &str, options: ExecuteRequestOptions) {
        trace_shell_request("execute_request", Some(code));
        self.send_execute_request(code, options);
    }

    /// Receive shell execute reply with tracing
    #[track_caller]
    pub fn recv_shell_execute_reply_traced(&self) -> u32 {
        let result = self.recv_shell_execute_reply();
        trace_shell_reply("execute_reply", "ok");
        result
    }

    /// Source a file that was created with `SourceFile::new()`.
    #[track_caller]
    pub fn source_file(&self, file: &SourceFile) {
        trace_separator(&format!("source({})", file.filename));
        self.send_execute_request(
            &format!("source('{}')", file.path),
            ExecuteRequestOptions::default(),
        );
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        // No execute result: source() returns invisibly
        self.recv_iopub_idle();
        self.recv_shell_execute_reply();
    }

    /// Execute code from a file with location information.
    ///
    /// This simulates running code from an editor where the frontend sends
    /// the file URI and position. Breakpoints in the code will be verified
    /// during execution.
    #[track_caller]
    pub fn execute_file(&self, file: &SourceFile) {
        let code = std::fs::read_to_string(&file.path).unwrap();
        self.send_execute_request(&code, ExecuteRequestOptions {
            positron: Some(ExecuteRequestPositron {
                code_location: Some(file.location()),
            }),
            ..Default::default()
        });
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_idle();
        self.recv_shell_execute_reply();
    }

    /// Source a file and stop at a breakpoint (set via DAP, not browser() in code).
    ///
    /// Source a file and wait until execution stops at an injected breakpoint.
    ///
    /// Use this when you've set breakpoints via `dap.set_breakpoints()` before sourcing.
    /// The caller must still receive the DAP events (see below) and should call
    /// `recv_shell_execute_reply()` after quitting the debugger.
    ///
    /// Due to the auto-stepping mechanism, hitting an injected breakpoint produces
    /// IOPub messages that may arrive in varying order and batching:
    /// - Stream output with "Called from:" and "debug at" (may be batched or separate)
    /// - start_debug / stop_debug comm messages
    /// - idle status
    ///
    /// **DAP events (caller must receive):**
    /// 1. Stopped (entering .ark_breakpoint)
    /// 2. Continued (auto-step triggered)
    /// 3. Continued (from stop_debug)
    /// 4. Stopped (at actual user expression)
    #[track_caller]
    pub fn source_file_and_hit_breakpoint(&self, file: &SourceFile) {
        trace_separator(&format!("source_and_hit_bp({}) START", file.filename));
        self.send_execute_request(
            &format!("source('{}')", file.path),
            ExecuteRequestOptions::default(),
        );
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        // Auto-stepping message flow when hitting an injected breakpoint.
        // Message count varies due to stream batching and timing. We collect
        // messages until we have evidence of all required events.
        self.recv_iopub_breakpoint_hit();
        trace_separator(&format!("source_and_hit_bp({}) END", file.filename));
    }

    /// Receive IOPub messages for a breakpoint hit with auto-stepping.
    ///
    /// Non-stream message sequence: start_debug, idle.
    /// Stream assertions: "Called from:" and "debug at".
    #[track_caller]
    pub fn recv_iopub_breakpoint_hit(&self) {
        trace_separator("recv_iopub_breakpoint_hit START");
        self.recv_iopub_start_debug();
        self.assert_stream_stdout_contains("Called from:");
        self.assert_stream_stdout_contains("debug at");
        self.recv_iopub_idle();
        trace_separator("recv_iopub_breakpoint_hit END");
    }

    /// Source a file that was created with `SourceFile::new()`.
    ///
    /// The code must contain `browser()` or a breakpoint to enter debug mode.
    /// The caller must still receive the DAP `Stopped` event.
    ///
    /// Non-stream message sequence: busy, execute_input, start_debug, idle, shell_reply.
    /// Stream assertion: "Called from:".
    #[track_caller]
    pub fn source_debug_file(&self, file: &SourceFile) {
        trace_separator(&format!("source_debug({})", file.filename));
        self.send_execute_request(
            &format!("source('{}')", file.path),
            ExecuteRequestOptions::default(),
        );
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_start_debug();
        self.assert_stream_stdout_contains("Called from:");
        self.recv_iopub_idle();
        self.recv_shell_execute_reply();
    }

    /// Source a file containing code that enters debug mode (e.g., via `browser()`).
    ///
    /// Returns a `SourceFile` containing the temp file (which must be kept alive)
    /// and the filename for use in assertions.
    ///
    /// The caller must still receive the DAP `Stopped` event.
    #[track_caller]
    pub fn send_source(&self, code: &str) -> SourceFile {
        let line_count = code.lines().count() as u32;
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{code}").unwrap();

        // Use forward slashes for R compatibility on Windows (backslashes would be
        // interpreted as escape sequences in R strings)
        let path = file.path().to_string_lossy().replace('\\', "/");
        let url = ExtUrl::from_file_path(file.path()).unwrap();
        let uri = url.to_string();
        let filename = file
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        self.send_execute_request(
            &format!("source('{path}')"),
            ExecuteRequestOptions::default(),
        );
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        self.recv_iopub_start_debug();
        self.assert_stream_stdout_contains("Called from:");
        self.recv_iopub_idle();
        self.recv_shell_execute_reply();

        SourceFile {
            file,
            path,
            filename,
            uri,
            line_count,
        }
    }

    /// Execute `browser()` and receive all expected messages.
    #[track_caller]
    pub fn debug_send_browser(&self) -> u32 {
        self.send_execute_request("browser()", ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_start_debug();
        self.recv_iopub_execute_result();
        self.recv_iopub_idle();
        self.recv_shell_execute_reply()
    }

    /// Execute `Q` to quit the browser and receive all expected messages.
    #[track_caller]
    pub fn debug_send_quit(&self) -> u32 {
        trace_separator("debug_send_quit START");
        self.send_execute_request("Q", ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_stop_debug();
        self.recv_iopub_idle();
        let result = self.recv_shell_execute_reply();
        trace_separator("debug_send_quit END");
        result
    }

    /// Execute `c` (continue) to next browser() breakpoint in a sourced file.
    ///
    /// When continuing from one browser() to another, R outputs "Called from:"
    /// instead of "debug at", so this needs a different message pattern.
    ///
    /// Non-stream message sequence: busy, execute_input, stop_debug, start_debug, idle, shell_reply.
    /// Stream assertion: "Called from:".
    #[track_caller]
    pub fn debug_send_continue_to_breakpoint(&self) -> u32 {
        self.send_execute_request("c", ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_stop_debug();
        self.recv_iopub_start_debug();
        self.assert_stream_stdout_contains("Called from:");
        self.recv_iopub_idle();
        self.recv_shell_execute_reply()
    }

    /// Execute an expression while in debug mode and receive all expected messages.
    ///
    /// This is for evaluating expressions that don't advance the debugger (e.g., `1`, `x`).
    /// The caller must still receive the DAP `Invalidated` event to refresh variables.
    ///
    /// Transient evals skip the stop_debug/start_debug cycle to preserve frame selection
    /// and keep frame IDs valid. The message sequence is (in order):
    /// 1. execute_result (the evaluated expression result)
    /// 2. idle
    #[track_caller]
    pub fn debug_send_expr(&self, expr: &str) -> u32 {
        self.send_execute_request(expr, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_execute_result();
        self.recv_iopub_idle();
        self.recv_shell_execute_reply()
    }

    /// Execute an expression that causes an error while in debug mode.
    ///
    /// Unlike stepping to an error (which exits debug), evaluating an error
    /// from the console should keep us in debug mode.
    /// The caller must still receive the DAP `Invalidated` event to refresh variables.
    ///
    /// The `error_contains` parameter specifies what substring to expect in the error.
    #[track_caller]
    pub fn debug_send_error_expr(&self, expr: &str, error_contains: &str) -> u32 {
        self.send_execute_request(expr, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        let evalue = self.recv_iopub_execute_error();
        assert!(
            evalue.contains(error_contains),
            "Expected error containing {error_contains:?}, got: {evalue:?}"
        );
        self.recv_iopub_idle();
        self.recv_shell_execute_reply_exception()
    }

    /// Execute a step command in a sourced file context.
    ///
    /// In sourced files with srcrefs, stepping produces additional messages compared
    /// to virtual document context: a `stop_debug` comm (debug session ends briefly),
    /// and a `Stream` with "debug at" output from R.
    ///
    /// The `file` parameter is used to assert that stdout contains "debug at {filename}".
    /// The caller must still consume DAP events (recv_continued, recv_stopped).
    #[track_caller]
    pub fn debug_send_step_command(&self, cmd: &str, file: &SourceFile) -> u32 {
        trace_separator(&format!("debug_step({})", cmd));
        self.send_execute_request(cmd, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_stop_debug();
        self.recv_iopub_start_debug();
        // Check both "debug at" and the filename appear (filename may have full path before it)
        self.assert_stream_stdout_contains("debug at");
        self.assert_stream_stdout_contains(&file.filename);
        self.recv_iopub_idle();
        self.recv_shell_execute_reply()
    }

    /// Execute a step command in a virtual document debug context.
    ///
    /// Unlike `debug_send_step_command` (which expects "debug at" stream output for
    /// file-backed sources), this handles the vdoc case where stepping doesn't
    /// produce "debug at" output.
    ///
    /// The caller must still consume DAP events (recv_continued, recv_stopped).
    #[track_caller]
    pub fn debug_send_vdoc_step_command(&self, cmd: &str) -> u32 {
        self.send_execute_request(cmd, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_stop_debug();
        self.recv_iopub_start_debug();
        self.drain_streams();
        self.recv_iopub_idle();
        self.recv_shell_execute_reply()
    }

    /// Execute code that enters a `debugonce()` function.
    ///
    /// This handles the transition from the current debug context into a function
    /// marked with `debugonce()`.
    ///
    /// Stream assertion: "debugging in:".
    /// The caller must still consume DAP events (recv_continued, recv_stopped).
    #[track_caller]
    pub fn debug_enter_debugonce(&self, code: &str) -> u32 {
        self.send_execute_request(code, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        if self.in_debug() {
            self.recv_iopub_stop_debug();
        }
        self.assert_stream_stdout_contains("debugging in:");
        self.recv_iopub_start_debug();
        self.recv_iopub_idle();
        self.recv_shell_execute_reply()
    }

    /// Step out of a vdoc function, returning to the parent debug context.
    ///
    /// When stepping out of a function, the return value appears as an `execute_result`.
    /// The caller must still consume DAP events (recv_continued, recv_stopped).
    #[track_caller]
    pub fn debug_send_vdoc_step_out(&self, cmd: &str) -> u32 {
        self.send_execute_request(cmd, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_stop_debug();
        self.recv_iopub_start_debug();
        self.recv_iopub_execute_result();
        self.drain_streams();
        self.recv_iopub_idle();
        self.recv_shell_execute_reply()
    }

    /// Finish execution of the current debug function when there is no parent
    /// debug context (i.e. debugonce was entered from top level, not from a browser).
    ///
    /// Unlike `debug_send_vdoc_step_out`, this does not expect a `start_debug`
    /// after `stop_debug`, since there is no parent debug session to return to.
    ///
    /// The caller must still consume the DAP `Continued` event (no `Stopped`).
    #[track_caller]
    pub fn debug_finish(&self, cmd: &str) -> u32 {
        self.send_execute_request(cmd, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_stop_debug();
        self.recv_iopub_execute_result();
        self.drain_streams();
        self.recv_iopub_idle();
        self.recv_shell_execute_reply()
    }

    /// Get the content of a virtual document by its URI.
    ///
    /// This queries the kernel's virtual document storage via an R function call.
    /// Returns `None` if the document is not found (or has empty content).
    ///
    /// Note: In debug mode, transient evals don't produce stop_debug/start_debug
    /// messages. The caller must receive the DAP `Invalidated` event separately.
    #[track_caller]
    pub fn get_virtual_document(&self, uri: &str) -> Option<String> {
        // Use cat() which handles NULL gracefully (outputs nothing)
        let code = format!(
            "cat(.ps.internal(.ps.Call(\"ps_get_virtual_document\", \"{}\")), sep = \"\\n\")",
            uri
        );
        self.send_execute_request(&code, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        // Wait for execution to complete. IOPub flushes streams before sending
        // Idle, so by collecting streams at Idle we ensure the output has arrived.
        let streams = self.recv_iopub_idle_and_flush();
        self.recv_shell_execute_reply();

        let content = streams.stdout.trim_end();
        if content.is_empty() {
            None
        } else {
            Some(content.to_string())
        }
    }
}

/// Result of sourcing a file via `send_source()`.
///
/// The temp file is kept alive as long as this struct exists.
pub struct SourceFile {
    file: NamedTempFile,
    pub path: String,
    pub filename: String,
    pub uri: String,
    line_count: u32,
}

impl SourceFile {
    /// Create a temp file with the given code without sourcing it.
    ///
    /// Use this when you need to set breakpoints before sourcing.
    /// After setting breakpoints, call `frontend.source_file()` to run the file.
    pub fn new(code: &str) -> Self {
        // Count lines for the location range
        let line_count = code.lines().count() as u32;
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{code}").unwrap();

        // Use forward slashes for R compatibility on Windows (backslashes would be
        // interpreted as escape sequences in R strings)
        let path = file.path().to_string_lossy().replace('\\', "/");
        let url = ExtUrl::from_file_path(file.path()).unwrap();
        let uri = url.to_string();

        // Extract file name
        let filename = file
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        Self {
            file,
            path,
            filename,
            uri,
            line_count,
        }
    }

    /// Get a `JupyterPositronLocation` pointing to this file.
    pub fn location(&self) -> JupyterPositronLocation {
        JupyterPositronLocation {
            uri: self.uri.clone(),
            range: JupyterPositronRange {
                start: JupyterPositronPosition {
                    line: 0,
                    character: 0,
                },
                end: JupyterPositronPosition {
                    line: self.line_count,
                    character: 0,
                },
            },
        }
    }

    /// Rewrite the file with new content.
    ///
    /// Use this for tests that need to modify the file after creation
    /// (e.g., testing hash change detection).
    pub fn rewrite(&mut self, code: &str) {
        self.file.rewind().unwrap();
        self.file.as_file_mut().set_len(0).unwrap();

        write!(self.file, "{code}").unwrap();

        self.file.flush().unwrap();
        self.line_count = code.lines().count() as u32;
    }
}
impl DummyArkFrontend {
    fn get_frontend() -> &'static Arc<Mutex<DummyFrontend>> {
        // These are the hard-coded defaults. Call `init()` explicitly to
        // override.
        let options = DummyArkFrontendOptions::default();
        FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyArkFrontend::init(options))))
    }

    fn init(options: DummyArkFrontendOptions) -> DummyFrontend {
        if FRONTEND.get().is_some() {
            panic!("Can't spawn Ark more than once");
        }

        // We don't want cli to try and restore the cursor, it breaks our tests
        // by adding unecessary ANSI escapes. We don't need this in Positron because
        // cli also checks `isatty(stdout())`, which is false in Positron because
        // we redirect stdout.
        // https://github.com/r-lib/cli/blob/1220ed092c03e167ff0062e9839c81d7258a4600/R/onload.R#L33-L40
        unsafe { std::env::set_var("R_CLI_HIDE_CURSOR", "false") };

        let connection = DummyConnection::new();
        let (connection_file, registration_file) = connection.get_connection_files();

        let mut r_args = vec![];

        // We aren't animals!
        r_args.push(String::from("--no-save"));
        r_args.push(String::from("--no-restore"));

        if options.interactive {
            r_args.push(String::from("--interactive"));
        }
        if !options.site_r_profile {
            r_args.push(String::from("--no-site-file"));
        }
        if !options.user_r_profile {
            r_args.push(String::from("--no-init-file"));
        }
        if !options.r_environ {
            r_args.push(String::from("--no-environ"));
        }

        // Start the kernel and REPL in a background thread, does not return and is never joined.
        // Must run `start_kernel()` in a background thread because it blocks until it receives
        // a `HandshakeReply`, which we send from `from_connection()` below.
        stdext::spawn!("dummy_kernel", move || {
            ark::start::start_kernel(
                connection_file,
                Some(registration_file),
                r_args,
                options.startup_file,
                options.session_mode,
                false,
                options.default_repos,
            );
        });

        DummyFrontend::from_connection(connection)
    }
}

// Check that we haven't left crumbs behind.
//
// Certain messages are allowed to remain because they can arrive asynchronously:
// - Stream messages: can interleave with other operations due to batching.
// - CommMsg with method "execute": `DapClient::drop()` calls `disconnect()` which
//   sends a Disconnect request. If ark is still debugging, `handle_disconnect()`
//   sends an `execute Q` comm message to quit the browser. Since `DapClient` is
//   dropped before `DummyArkFrontend` (reverse declaration order), this message
//   can arrive here after the test has otherwise completed cleanly.
impl Drop for DummyArkFrontend {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }

        // Drain any straggler streams
        let drained = self.drain_streams_internal();
        let has_streams = !drained.stdout.is_empty() || !drained.stderr.is_empty();

        // Fail if streams were received but no stream assertions were made
        if has_streams && !self.streams_handled.get() {
            panic!(
                "Test received stream output but made no stream assertions.\n\
                 stdout: {:?}\n\
                 stderr: {:?}",
                drained.stdout, drained.stderr
            );
        }

        // Fail if variables events were buffered but never consumed
        let buffered_variables = self.variables_events.borrow();
        if !buffered_variables.is_empty() {
            panic!(
                "Test has {} unconsumed variables event(s): {:?}",
                buffered_variables.len(),
                *buffered_variables
            );
        }
        drop(buffered_variables);

        self.data_explorer.assert_consumed();

        // Helper to check if a message is exempt from "unexpected message" check
        let is_exempt = |msg: &Message| -> bool {
            match msg {
                Message::Stream(_) => true,
                Message::CommMsg(comm) => {
                    comm.content.data.get("method").and_then(|v| v.as_str()) == Some("execute") &&
                        comm.content
                            .data
                            .get("params")
                            .and_then(|p| p.get("command"))
                            .and_then(|c| c.as_str()) ==
                            Some("Q")
                },
                _ => false,
            }
        };

        // Drain any pending IOPub messages (including those in our put-back queue)
        let mut unexpected_messages: Vec<Message> = self
            .pending_iopub_messages
            .borrow_mut()
            .drain(..)
            .filter(|msg| !is_exempt(msg))
            .collect();

        while self.iopub_socket.has_incoming_data().unwrap() {
            let msg = Message::read_from_socket(&self.iopub_socket).unwrap();
            if !is_exempt(&msg) {
                unexpected_messages.push(msg);
            }
        }

        // Fail if any unexpected IOPub messages were left behind
        if !unexpected_messages.is_empty() {
            panic!(
                "IOPub socket has {} unexpected message(s) on exit:\n{:#?}",
                unexpected_messages.len(),
                unexpected_messages
            );
        }

        // Check other sockets strictly (no leniency for non-IOPub)
        let mut shell_messages: Vec<Message> = Vec::new();
        let mut stdin_messages: Vec<Message> = Vec::new();

        while self.shell_socket.has_incoming_data().unwrap() {
            if let Ok(msg) = Message::read_from_socket(&self.shell_socket) {
                shell_messages.push(msg);
            }
        }
        while self.stdin_socket.has_incoming_data().unwrap() {
            if let Ok(msg) = Message::read_from_socket(&self.stdin_socket) {
                stdin_messages.push(msg);
            }
        }

        if !shell_messages.is_empty() || !stdin_messages.is_empty() {
            panic!(
                "Non-IOPub sockets have unexpected messages on exit:\n\
                 Shell: {:#?}\n\
                 StdIn: {:#?}",
                shell_messages, stdin_messages
            );
        }
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyArkFrontend {
    type Target = DummyFrontend;

    fn deref(&self) -> &Self::Target {
        Deref::deref(&self.guard)
    }
}

impl DerefMut for DummyArkFrontend {
    fn deref_mut(&mut self) -> &mut Self::Target {
        DerefMut::deref_mut(&mut self.guard)
    }
}

impl DummyArkFrontendNotebook {
    /// Lock a notebook frontend.
    ///
    /// NOTE: Only one `DummyArkFrontend` variant should call `lock()` within
    /// a given process.
    pub fn lock() -> Self {
        Self::init();

        Self {
            inner: DummyArkFrontend::lock(),
        }
    }

    /// Initialize with Notebook session mode
    fn init() {
        let mut options = DummyArkFrontendOptions::default();
        options.session_mode = SessionMode::Notebook;
        FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyArkFrontend::init(options))));
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyArkFrontendNotebook {
    type Target = DummyArkFrontend;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for DummyArkFrontendNotebook {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl DummyArkFrontendDefaultRepos {
    /// Lock a frontend with a default repos setting.
    ///
    /// NOTE: `startup_file` is required because you typically want
    /// to force `options(repos =)` to a fixed value for testing, regardless
    /// of what the caller's default `repos` are set as (i.e. rig typically
    /// sets it to a non-`@CRAN@` value).
    ///
    /// NOTE: Only one `DummyArkFrontend` variant should call `lock()` within
    /// a given process.
    pub fn lock(default_repos: DefaultRepos, startup_file: String) -> Self {
        Self::init(default_repos, startup_file);

        Self {
            inner: DummyArkFrontend::lock(),
        }
    }

    /// Initialize with given default repos
    fn init(default_repos: DefaultRepos, startup_file: String) {
        let mut options = DummyArkFrontendOptions::default();
        options.default_repos = default_repos;
        options.startup_file = Some(startup_file);

        FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyArkFrontend::init(options))));
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyArkFrontendDefaultRepos {
    type Target = DummyArkFrontend;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
impl DummyArkFrontendRprofile {
    /// Lock a frontend that supports `.Rprofile`s.
    ///
    /// NOTE: This variant can only be called exactly once per process,
    /// because you can only load an `.Rprofile` one time. Additionally,
    /// only one `DummyArkFrontend` variant should call `lock()` within
    /// a given process. Practically, this ends up meaning you can only
    /// have 1 test block per integration test that uses a
    /// `DummyArkFrontendRprofile`.
    pub fn lock() -> Self {
        Self::init();

        Self {
            inner: DummyArkFrontend::lock(),
        }
    }

    /// Initialize with user level `.Rprofile` enabled
    fn init() {
        let mut options = DummyArkFrontendOptions::default();
        options.user_r_profile = true;
        let status = FRONTEND.set(Arc::new(Mutex::new(DummyArkFrontend::init(options))));

        if status.is_err() {
            panic!("You can only call `DummyArkFrontendRprofile::lock()` once per process.");
        }

        FRONTEND.get().unwrap();
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyArkFrontendRprofile {
    type Target = DummyArkFrontend;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for DummyArkFrontendRprofile {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Default for DummyArkFrontendOptions {
    fn default() -> Self {
        Self {
            interactive: true,
            site_r_profile: false,
            user_r_profile: false,
            r_environ: false,
            session_mode: SessionMode::Console,
            default_repos: DefaultRepos::Auto,
            startup_file: None,
        }
    }
}
