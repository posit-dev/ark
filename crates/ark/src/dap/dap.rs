//
// dap.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::server_comm::ServerStartMessage;
use amalthea::comm::server_comm::ServerStartedMessage;
use amalthea::language::server_handler::ServerHandler;
use amalthea::socket::comm::CommOutgoingTx;
use anyhow::anyhow;
use crossbeam::channel::Sender;
use dap::responses::EvaluateResponse;
use dap::types::Variable;
use harp::environment::R_ENVS;
use harp::object::RObject;
use stdext::result::ResultExt;
use stdext::spawn;
use url::Url;

use crate::console::ConsoleOutputCapture;
use crate::console::DebugStoppedReason;
use crate::console_debug::FrameInfo;
use crate::dap::dap_server;
use crate::dap::dap_variables::object_variable;
use crate::dap::dap_variables::RVariable;
use crate::request::RRequest;
use crate::thread::RThreadSafe;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakpointState {
    Unverified,
    Verified,
    Invalid(InvalidReason),
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvalidReason {
    ClosingBrace,
    EmptyBraces,
}

impl InvalidReason {
    pub fn message(&self) -> &'static str {
        match self {
            InvalidReason::ClosingBrace => "Can't break on closing `}` brace",
            InvalidReason::EmptyBraces => "Can't break inside empty braces",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Breakpoint {
    pub id: i64,
    /// The line where the breakpoint is actually placed (may be anchored to expression start).
    /// 0-based.
    pub line: u32,
    /// The line originally requested by the user (before anchoring). Used to match breakpoints
    /// across SetBreakpoints requests. 0-based.
    pub original_line: u32,
    pub state: BreakpointState,
    /// Whether this breakpoint was actually injected into code during annotation.
    /// Only injected breakpoints can be verified by range-based verification.
    pub injected: bool,
}

impl Breakpoint {
    /// Create a new breakpoint. The `original_line` is set to the same as `line`.
    pub fn new(id: i64, line: u32, state: BreakpointState) -> Self {
        Self {
            id,
            line,
            original_line: line,
            state,
            injected: false,
        }
    }

    /// Convert from DAP 1-based line to internal 0-based line
    pub fn from_dap_line(line: i64) -> u32 {
        (line - 1) as u32
    }

    /// Convert from internal 0-based line to DAP 1-based line
    pub fn to_dap_line(line: u32) -> i64 {
        (line + 1) as i64
    }
}

#[derive(Debug, Clone)]
pub enum DapBackendEvent {
    /// Event sent when a normal (non-browser) prompt marks the end of a
    /// debugging session.
    Terminated,

    /// Event sent when user types `n`, `f`, `c`, or `cont`.
    Continued,

    /// Event sent when a browser prompt is emitted during an existing
    /// debugging session
    Stopped,

    /// Event sent after a console evaluation so the frontend refreshes
    /// variables .
    Invalidated,

    /// Event sent when a breakpoint state changes (verified, unverified, or invalid)
    /// The line is included so the frontend can update the breakpoint's position
    /// (e.g., when a breakpoint inside a multiline expression anchors to its start)
    /// The message is included for invalid breakpoints to explain why.
    BreakpointState {
        id: i64,
        line: u32,
        verified: bool,
        message: Option<String>,
    },

    /// Event sent when an exception/error occurs
    Exception(DapExceptionEvent),
}

#[derive(Debug, Clone)]
pub struct DapExceptionEvent {
    pub class: String,
    pub message: String,
}

pub struct Dap {
    /// Whether the REPL is stopped with a browser prompt.
    pub is_debugging: bool,

    /// Whether the DAP server is connected to a client.
    pub is_connected: bool,

    /// Channel for sending events to the DAP frontend.
    /// This always exists when `is_connected` is true.
    pub backend_events_tx: Option<Sender<DapBackendEvent>>,

    /// Current call stack
    pub stack: Option<Vec<FrameInfo>>,

    /// Known breakpoints keyed by URI, with document hash
    pub breakpoints: HashMap<Url, (blake3::Hash, Vec<Breakpoint>)>,

    /// Filters for enabled condition breakpoints
    pub exception_breakpoint_filters: Vec<String>,

    /// Map of `source` -> `source_reference` used for frames that don't have
    /// associated files (i.e. no `srcref` attribute). The `source` is the key to
    /// ensure that we don't insert the same function multiple times, which would result
    /// in duplicate virtual editors being opened on the client side.
    pub fallback_sources: HashMap<String, String>,

    /// Maps a frame `id` from within the `stack` to a unique
    /// `variables_reference` id, which then allows you to use
    /// `variables_reference_to_r_object` to look up the R object to collect
    /// variables from. Reset after each debug step.
    pub frame_id_to_variables_reference: HashMap<i64, i64>,

    /// Maps a `variables_reference` to the corresponding R object used to
    /// collect variables from. The R object may be a frame environment from
    /// a `FrameInfo`, or an arbitrarily nested child of one of those
    /// environments if the child has its own children. Reset after each debug step,
    /// allowing us to free our references to the R objects.
    pub variables_reference_to_r_object: HashMap<i64, RThreadSafe<RObject>>,

    /// The current `variables_reference`. Unique within a debug session. Reset after
    /// `stop_debug()`, not between debug steps like the hash maps are. If we reset
    /// between steps, we could potentially have a race condition where
    /// `handle_variables()` could request `variables` for a `variables_reference` that
    /// we've already overwritten the R object for, potentially sending back incorrect
    /// information.
    current_variables_reference: i64,

    /// Monotonically increasing breakpoint ID counter
    current_breakpoint_id: i64,

    /// Whether an interrupt was sent to drop into the debugger
    pub(crate) is_interrupting_for_debugger: bool,

    /// Channel for sending events to the comm frontend.
    comm_tx: Option<CommOutgoingTx>,

    /// Channel for sending debug commands to `read_console()`
    r_request_tx: Sender<RRequest>,

    /// Self-reference under a mutex. Shared with the R, Shell socket, and
    /// DAP server threads.
    shared_self: Option<Arc<Mutex<Dap>>>,
}

impl Dap {
    pub fn new_shared(r_request_tx: Sender<RRequest>) -> Arc<Mutex<Self>> {
        let state = Self {
            is_debugging: false,
            is_connected: false,
            backend_events_tx: None,
            stack: None,
            breakpoints: HashMap::new(),
            exception_breakpoint_filters: Vec::new(),
            fallback_sources: HashMap::new(),
            frame_id_to_variables_reference: HashMap::new(),
            variables_reference_to_r_object: HashMap::new(),
            current_variables_reference: 1,
            current_breakpoint_id: 1,
            comm_tx: None,
            r_request_tx,
            shared_self: None,
            is_interrupting_for_debugger: false,
        };

        let shared = Arc::new(Mutex::new(state));

        // Set shareable self-reference
        {
            let mut state = shared.lock().unwrap();
            state.shared_self = Some(shared.clone());
        }

        shared
    }

    /// Notify the frontend that we've entered the debugger.
    ///
    /// The DAP session is expected to always be connected (to receive breakpoint
    /// updates). The `start_debug` comm message is a hint for the frontend to
    /// show the debug toolbar, not a session lifecycle event.
    pub fn start_debug(
        &mut self,
        mut stack: Vec<FrameInfo>,
        fallback_sources: HashMap<String, String>,
        stopped_reason: DebugStoppedReason,
    ) {
        self.is_debugging = true;
        self.fallback_sources.extend(fallback_sources);

        self.load_variables_references(&mut stack);
        self.stack = Some(stack);

        log::trace!("DAP: Sending `start_debug` events");

        if let Some(comm_tx) = &self.comm_tx {
            // Ask frontend to connect to the DAP
            comm_tx
                .send(amalthea::comm_rpc_message!("start_debug"))
                .log_err();

            if let Some(dap_tx) = &self.backend_events_tx {
                let event = match stopped_reason {
                    DebugStoppedReason::Step | DebugStoppedReason::Pause => {
                        DapBackendEvent::Stopped
                    },
                    DebugStoppedReason::Condition { class, message } => {
                        DapBackendEvent::Exception(DapExceptionEvent { class, message })
                    },
                };
                dap_tx.send(event).log_err();
            }
        }
    }

    /// Notify the frontend that we've exited the debugger.
    ///
    /// The DAP session remains connected. The `stop_debug` comm message is a
    /// hint for the frontend to hide the debug toolbar. We send `Continued`
    /// (not `Terminated`) so the DAP connection stays active for receiving
    /// breakpoint updates.
    pub fn stop_debug(&mut self) {
        // Reset state
        self.stack = None;

        // Fallback reset in case the interrupt was caught before our global
        // calling handler could consume it (e.g., a `tryCatch(interrupt = )`
        // around the interrupted code).
        self.is_interrupting_for_debugger = false;
        self.fallback_sources.clear();
        self.clear_variables_reference_maps();
        self.reset_variables_reference_count();

        let was_debugging = self.is_debugging;
        self.is_debugging = false;

        if was_debugging && self.is_connected {
            log::trace!("DAP: Sending `stop_debug` events");

            if let Some(comm_tx) = &self.comm_tx {
                comm_tx
                    .send(amalthea::comm_rpc_message!("stop_debug"))
                    .log_err();

                if let Some(datp_tx) = &self.backend_events_tx {
                    datp_tx.send(DapBackendEvent::Continued).log_err();
                }
            }
            // else: If not connected to a frontend, the DAP client should
            // have received a `Continued` event already, after a `n`
            // command or similar.
        }
    }

    pub fn send_invalidated(&self) {
        if let Some(tx) = &self.backend_events_tx {
            tx.send(DapBackendEvent::Invalidated).log_err();
        }
    }

    fn load_variables_references(&mut self, stack: &mut Vec<FrameInfo>) {
        // Reset the last step's maps. The frontend should never ask for these variable
        // references or variables again (and if it does due to some race condition, we
        // end up replying with an error). This lets us free our references to the
        // R objects used to populate the variables pane between steps.
        self.clear_variables_reference_maps();

        for frame in stack.iter_mut() {
            // Move the `environment` out of the `FrameInfo`, who's only
            // job is to get it here. We don't use it otherwise.
            // If the frame has no environment (e.g. top-level browser), use global env.
            let environment = frame
                .environment
                .take()
                .unwrap_or_else(|| RThreadSafe::new(RObject::new(R_ENVS.global)));

            // Map this frame's `id` to a unique `variables_reference`, and
            // then map that `variables_reference` to the R object we will
            // eventually get the variables from
            self.frame_id_to_variables_reference
                .insert(frame.id, self.current_variables_reference);
            self.variables_reference_to_r_object
                .insert(self.current_variables_reference, environment);

            self.current_variables_reference += 1;
        }
    }

    // Called between steps
    fn clear_variables_reference_maps(&mut self) {
        self.frame_id_to_variables_reference.clear();
        self.variables_reference_to_r_object.clear();
    }

    // Called between debug sessions (i.e. on `debug_stop()`)
    fn reset_variables_reference_count(&mut self) {
        self.current_variables_reference = 1;
    }

    /// Map an arbitrary `RObject` to a new unique `variables_reference`
    ///
    /// This is used on structured R objects that have children requiring a
    /// lazy secondary `Variables` request to collect those children.
    ///
    /// Returns the `variables_reference` which gets bound to the corresponding
    /// `Variable` object for `x`, which the frontend uses to request its
    /// children.
    pub fn insert_variables_reference_object(&mut self, x: RThreadSafe<RObject>) -> i64 {
        let variables_reference = self.current_variables_reference;

        self.variables_reference_to_r_object
            .insert(variables_reference, x);
        self.current_variables_reference += 1;

        variables_reference
    }

    pub fn into_variables(&mut self, variables: Vec<RVariable>) -> Vec<Variable> {
        let mut out = Vec::with_capacity(variables.len());

        for variable in variables.into_iter() {
            // If we have a `variables_reference_object`, then this variable is
            // structured and has children. We need a new unique
            // `variables_reference` to return that will map to this object in
            // a followup `Variables` request.
            let variables_reference = match variable.variables_reference_object {
                Some(x) => self.insert_variables_reference_object(x),
                None => 0,
            };

            let variable = Variable {
                name: variable.name,
                value: variable.value,
                type_field: variable.type_field,
                presentation_hint: None,
                evaluate_name: None,
                variables_reference,
                named_variables: None,
                indexed_variables: None,
                memory_reference: None,
            };

            out.push(variable);
        }

        out
    }

    pub fn into_evaluate_response(&mut self, variable: RVariable) -> EvaluateResponse {
        let variables_reference = match variable.variables_reference_object {
            Some(obj) => self.insert_variables_reference_object(obj),
            None => 0,
        };

        EvaluateResponse {
            result: variable.value,
            type_field: variable.type_field,
            presentation_hint: None,
            variables_reference,
            named_variables: None,
            indexed_variables: None,
            memory_reference: None,
        }
    }

    pub fn evaluate(
        &self,
        expression: &str,
        frame_id: Option<i64>,
        capture: Option<&mut ConsoleOutputCapture>,
    ) -> Result<RVariable, String> {
        let env = self.frame_env(frame_id)?;

        match harp::parse_eval0(expression, harp::RObject::view(env)) {
            Ok(value) => {
                if let Some(capture) = capture {
                    harp::utils::r_print(value.sexp);
                    Ok(RVariable {
                        name: String::new(),
                        value: capture.take().trim_end().to_string(),
                        type_field: None,
                        variables_reference_object: None,
                    })
                } else {
                    Ok(object_variable(String::new(), value.sexp))
                }
            },
            Err(err) => Err(evaluate_error_message(err)),
        }
    }

    pub(crate) fn frame_env(&self, frame_id: Option<i64>) -> Result<libr::SEXP, String> {
        let Some(frame_id) = frame_id else {
            return Ok(R_ENVS.global);
        };

        let Some(variables_reference) =
            self.frame_id_to_variables_reference.get(&frame_id).copied()
        else {
            return Err(format!("Unknown `frame_id`: {frame_id}"));
        };

        let Some(obj) = self
            .variables_reference_to_r_object
            .get(&variables_reference)
        else {
            return Err(format!(
                "Unknown `variables_reference`: {variables_reference}"
            ));
        };

        Ok(obj.get().sexp)
    }

    pub fn next_breakpoint_id(&mut self) -> i64 {
        let id = self.current_breakpoint_id;
        self.current_breakpoint_id += 1;
        id
    }

    pub fn is_exception_breakpoint_filter_enabled(&self, filter: &str) -> bool {
        self.exception_breakpoint_filters
            .iter()
            .any(|enabled| enabled == filter)
    }

    /// Verify breakpoints within a line range for a given URI
    ///
    /// Loops over all breakpoints for the URI and verifies any unverified
    /// breakpoints that fall within the range [start_line, end_line).
    /// Sends a `BreakpointVerified` event for each newly verified breakpoint.
    pub fn verify_breakpoints(&mut self, uri: &Url, start_line: u32, end_line: u32) {
        let Some((_, bp_list)) = self.breakpoints.get_mut(uri) else {
            return;
        };

        for bp in bp_list.iter_mut() {
            // Verified and Disabled breakpoints are both already verified.
            // Invalid breakpoints never get verified so we skip them too.
            // Only injected breakpoints can be verified by range. Non-injected
            // breakpoints were added by the user after the code was parsed and
            // remain unverified until re-parsing / re-evaluation.
            if matches!(
                bp.state,
                BreakpointState::Verified | BreakpointState::Disabled | BreakpointState::Invalid(_)
            ) || !bp.injected
            {
                continue;
            }

            let line = bp.line;
            if line >= start_line && line < end_line {
                bp.state = BreakpointState::Verified;

                if let Some(tx) = &self.backend_events_tx {
                    tx.send(DapBackendEvent::BreakpointState {
                        id: bp.id,
                        line: bp.line,
                        verified: true,
                        message: None,
                    })
                    .log_err();
                }
            }
        }
    }

    /// Verify a single breakpoint by ID
    ///
    /// Finds the breakpoint with the given ID for the URI and marks it as verified
    /// if it was previously unverified. Sends a `BreakpointVerified` event.
    pub fn verify_breakpoint(&mut self, uri: &Url, id: &str) {
        let Some((_, bp_list)) = self.breakpoints.get_mut(uri) else {
            return;
        };
        let Some(bp) = bp_list.iter_mut().find(|bp| bp.id.to_string() == id) else {
            return;
        };

        // Only verify unverified breakpoints
        if !matches!(bp.state, BreakpointState::Unverified) {
            return;
        }

        bp.state = BreakpointState::Verified;

        if let Some(tx) = &self.backend_events_tx {
            tx.send(DapBackendEvent::BreakpointState {
                id: bp.id,
                line: bp.line,
                verified: true,
                message: None,
            })
            .log_err();
        }
    }

    /// Called when a document changes. Removes all breakpoints for the URI
    /// and sends unverified events for each one.
    pub fn did_change_document(&mut self, uri: &Url) {
        let Some((_, breakpoints)) = self.breakpoints.remove(uri) else {
            return;
        };
        let Some(tx) = &self.backend_events_tx else {
            return;
        };

        for bp in breakpoints {
            tx.send(DapBackendEvent::BreakpointState {
                id: bp.id,
                line: bp.line,
                verified: false,
                message: None,
            })
            .log_err();
        }
    }

    /// Notify the frontend about breakpoints that were marked invalid during annotation.
    /// Sends a `BreakpointState` event with verified=false and a message for each.
    pub fn notify_invalid_breakpoints(&self, uri: &Url) {
        let Some(tx) = &self.backend_events_tx else {
            return;
        };
        let Some((_, breakpoints)) = self.breakpoints.get(uri) else {
            return;
        };

        for bp in breakpoints {
            let BreakpointState::Invalid(reason) = &bp.state else {
                continue;
            };
            tx.send(DapBackendEvent::BreakpointState {
                id: bp.id,
                line: bp.line,
                verified: false,
                message: Some(reason.message().to_string()),
            })
            .log_err();
        }
    }

    /// Remove disabled breakpoints for a given URI.
    pub fn remove_disabled_breakpoints(&mut self, uri: &Url) {
        let Some((_, bps)) = self.breakpoints.get_mut(uri) else {
            return;
        };
        bps.retain(|bp| !matches!(bp.state, BreakpointState::Disabled));
    }

    pub(crate) fn is_breakpoint_enabled(&self, uri: &Url, id: String) -> bool {
        let Some((_, breakpoints)) = self.breakpoints.get(uri) else {
            return false;
        };

        // Unverified breakpoints are enabled. This happens when we hit a
        // breakpoint in an expression that hasn't been evaluated yet (or hasn't
        // finished).
        breakpoints.iter().any(|bp| {
            bp.id.to_string() == id &&
                matches!(
                    bp.state,
                    BreakpointState::Verified | BreakpointState::Unverified
                )
        })
    }

    pub fn get_frame_env(&self, frame_id: Option<i64>) -> anyhow::Result<libr::SEXP> {
        let Some(frame_id) = frame_id else {
            return Ok(R_ENVS.global);
        };

        let Some(variables_reference) =
            self.frame_id_to_variables_reference.get(&frame_id).copied()
        else {
            return Err(anyhow!("Unknown `frame_id`: {frame_id}"));
        };

        let Some(obj) = self
            .variables_reference_to_r_object
            .get(&variables_reference)
        else {
            return Err(anyhow!(
                "Unknown `variables_reference`: {variables_reference}"
            ));
        };

        Ok(obj.get().sexp)
    }
}

fn evaluate_error_message(err: harp::Error) -> String {
    match err {
        harp::Error::TryCatchError { message, .. } => message,
        err => format!("{err}"),
    }
}

// Handler for Amalthea socket threads
impl ServerHandler for Dap {
    fn start(
        &mut self,
        server_start: ServerStartMessage,
        server_started_tx: Sender<ServerStartedMessage>,
        comm_tx: CommOutgoingTx,
    ) -> Result<(), amalthea::error::Error> {
        log::info!("DAP: Spawning thread");

        // If `start()` is called we are now connected to a frontend
        self.comm_tx = Some(comm_tx.clone());

        // Create the DAP thread that manages connections and creates a
        // server when connected. This is currently the only way to create
        // this thread but in the future we might provide other ways to
        // connect to the DAP without a Jupyter comm.
        let r_request_tx_clone = self.r_request_tx.clone();

        // This can't panic as `Dap` can't be constructed without a shared self
        let state_clone = self.shared_self.as_ref().unwrap().clone();

        spawn!("ark-dap", move || {
            dap_server::start_dap(
                state_clone,
                server_start,
                server_started_tx,
                r_request_tx_clone,
                comm_tx,
            )
        });

        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use crossbeam::channel::unbounded;

    use super::*;

    fn create_test_dap() -> (Dap, crossbeam::channel::Receiver<DapBackendEvent>) {
        let (backend_events_tx, backend_events_rx) = unbounded();
        let (r_request_tx, _r_request_rx) = unbounded();
        let dap = Dap {
            is_debugging: false,
            is_connected: true,
            backend_events_tx: Some(backend_events_tx),
            stack: None,
            breakpoints: HashMap::new(),
            exception_breakpoint_filters: Vec::new(),
            fallback_sources: HashMap::new(),
            frame_id_to_variables_reference: HashMap::new(),
            variables_reference_to_r_object: HashMap::new(),
            current_variables_reference: 1,
            current_breakpoint_id: 1,
            is_interrupting_for_debugger: false,
            comm_tx: None,
            r_request_tx,
            shared_self: None,
        };

        (dap, backend_events_rx)
    }

    #[test]
    fn test_did_change_document_removes_breakpoints() {
        let (mut dap, rx) = create_test_dap();

        let uri = Url::parse("file:///test.R").unwrap();
        let hash = blake3::hash(b"test content");

        dap.breakpoints.insert(
            uri.clone(),
            (hash, vec![
                Breakpoint::new(1, 10, BreakpointState::Verified),
                Breakpoint::new(2, 20, BreakpointState::Verified),
            ]),
        );

        dap.did_change_document(&uri);

        assert!(dap.breakpoints.get(&uri).is_none());

        let event1 = rx.try_recv().unwrap();
        let event2 = rx.try_recv().unwrap();

        assert!(matches!(event1, DapBackendEvent::BreakpointState {
            id: 1,
            verified: false,
            ..
        }));
        assert!(matches!(event2, DapBackendEvent::BreakpointState {
            id: 2,
            verified: false,
            ..
        }));

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_did_change_document_no_breakpoints_is_noop() {
        let (mut dap, rx) = create_test_dap();

        let uri = Url::parse("file:///test.R").unwrap();

        dap.did_change_document(&uri);

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_did_change_document_only_affects_target_uri() {
        let (mut dap, rx) = create_test_dap();

        let uri1 = Url::parse("file:///test1.R").unwrap();
        let uri2 = Url::parse("file:///test2.R").unwrap();
        let hash1 = blake3::hash(b"content 1");
        let hash2 = blake3::hash(b"content 2");

        dap.breakpoints.insert(
            uri1.clone(),
            (hash1, vec![Breakpoint::new(
                1,
                10,
                BreakpointState::Verified,
            )]),
        );
        dap.breakpoints.insert(
            uri2.clone(),
            (hash2, vec![Breakpoint::new(
                2,
                20,
                BreakpointState::Verified,
            )]),
        );

        dap.did_change_document(&uri1);

        assert!(dap.breakpoints.get(&uri1).is_none());
        assert!(dap.breakpoints.get(&uri2).is_some());

        let event = rx.try_recv().unwrap();
        assert!(matches!(event, DapBackendEvent::BreakpointState {
            id: 1,
            ..
        }));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_did_change_document_without_backend_tx_is_noop() {
        let (r_request_tx, _r_request_rx) = unbounded();

        let mut dap = Dap {
            is_debugging: false,
            is_connected: false,
            backend_events_tx: None,
            stack: None,
            breakpoints: HashMap::new(),
            exception_breakpoint_filters: Vec::new(),
            fallback_sources: HashMap::new(),
            frame_id_to_variables_reference: HashMap::new(),
            variables_reference_to_r_object: HashMap::new(),
            current_variables_reference: 1,
            current_breakpoint_id: 1,
            is_interrupting_for_debugger: false,
            comm_tx: None,
            r_request_tx,
            shared_self: None,
        };

        let uri = Url::parse("file:///test.R").unwrap();
        let hash = blake3::hash(b"test content");

        dap.breakpoints.insert(
            uri.clone(),
            (hash, vec![Breakpoint::new(
                1,
                10,
                BreakpointState::Verified,
            )]),
        );

        // Should not panic even without `backend_events_tx`
        dap.did_change_document(&uri);

        // Breakpoints should still be removed
        assert!(dap.breakpoints.get(&uri).is_none());
    }
}
