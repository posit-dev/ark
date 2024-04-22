//
// dap.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use amalthea::comm::comm_channel::CommMsg;
use amalthea::language::server_handler::ServerHandler;
use crossbeam::channel::Sender;
use harp::object::RObject;
use serde_json::json;
use stdext::log_error;
use stdext::spawn;

use crate::dap::dap_r_main::FrameInfo;
use crate::dap::dap_r_main::FrameSource;
use crate::dap::dap_server;
use crate::request::RRequest;
use crate::thread::RThreadSafe;

#[derive(Debug, Copy, Clone)]
pub enum DapBackendEvent {
    /// Event sent when a normal (non-browser) prompt marks the end of a
    /// debugging session.
    Terminated,

    /// Event sent when user types `n`, `f`, `c`, or `cont`.
    Continued,

    /// Event sent when a browser prompt is emitted during an existing
    /// debugging session
    Stopped,
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

    /// Map of `source` -> `source_reference` used for frames that don't have
    /// associated files (i.e. no `srcref` attribute). The `source` is the key to
    /// ensure that we don't insert the same function multiple times, which would result
    /// in duplicate virtual editors being opened on the client side.
    pub fallback_sources: HashMap<String, i32>,
    current_source_reference: i32,

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

    /// Channel for sending events to the comm frontend.
    comm_tx: Option<Sender<CommMsg>>,

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
            fallback_sources: HashMap::new(),
            current_source_reference: 1,
            frame_id_to_variables_reference: HashMap::new(),
            variables_reference_to_r_object: HashMap::new(),
            current_variables_reference: 1,
            comm_tx: None,
            r_request_tx,
            shared_self: None,
        };

        let shared = Arc::new(Mutex::new(state));

        // Set shareable self-reference
        {
            let mut state = shared.lock().unwrap();
            state.shared_self = Some(shared.clone());
        }

        shared
    }

    pub fn start_debug(&mut self, mut stack: Vec<FrameInfo>) {
        self.load_fallback_sources(&stack);
        self.load_variables_references(&mut stack);
        self.stack = Some(stack);

        if self.is_debugging {
            if let Some(tx) = &self.backend_events_tx {
                log_error!(tx.send(DapBackendEvent::Stopped));
            }
        } else {
            if let Some(tx) = &self.comm_tx {
                // Ask frontend to connect to the DAP
                log::trace!("DAP: Sending `start_debug` event");
                let msg = CommMsg::Data(json!({
                    "msg_type": "start_debug",
                    "content": {}
                }));
                log_error!(tx.send(msg));
            }

            self.is_debugging = true;
        }
    }

    pub fn stop_debug(&mut self) {
        // Reset state
        self.stack = None;
        self.clear_fallback_sources();
        self.clear_variables_reference_maps();
        self.reset_variables_reference_count();
        self.is_debugging = false;

        if self.is_connected {
            if let Some(_) = &self.comm_tx {
                // Let frontend know we've quit the debugger so it can
                // terminate the debugging session and disconnect.
                if let Some(tx) = &self.backend_events_tx {
                    log::trace!("DAP: Sending `stop_debug` event");
                    log_error!(tx.send(DapBackendEvent::Terminated));
                }
            }
            // else: If not connected to a frontend, the DAP client should
            // have received a `Continued` event already, after a `n`
            // command or similar.
        }
    }

    /// Load `fallback_sources` with this stack's text sources
    fn load_fallback_sources(&mut self, stack: &Vec<FrameInfo>) {
        for frame in stack.iter() {
            let source = &frame.source;

            match source {
                FrameSource::File(_) => continue,
                FrameSource::Text(source) => {
                    if self.fallback_sources.contains_key(source) {
                        // Already in `fallback_sources`, associated with an existing `source_reference`
                        continue;
                    }
                    self.fallback_sources
                        .insert(source.clone(), self.current_source_reference);
                    self.current_source_reference += 1;
                },
            }
        }
    }

    fn clear_fallback_sources(&mut self) {
        self.fallback_sources.clear();
        self.current_source_reference = 1;
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
            let environment = frame.environment.take();

            let Some(environment) = environment else {
                continue;
            };

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
}

// Handler for Amalthea socket threads
impl ServerHandler for Dap {
    fn start(
        &mut self,
        tcp_address: String,
        conn_init_tx: Sender<bool>,
        comm_tx: Sender<CommMsg>,
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
                tcp_address,
                state_clone,
                conn_init_tx,
                r_request_tx_clone,
                comm_tx,
            )
        });

        return Ok(());
    }
}
