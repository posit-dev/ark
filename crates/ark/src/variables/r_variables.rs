//
// r_variables.rs
//
// Copyright (C) 2023-2024 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::comm::variables_comm::ClipboardFormatFormat;
use amalthea::comm::variables_comm::FormattedVariable;
use amalthea::comm::variables_comm::InspectedVariable;
use amalthea::comm::variables_comm::RefreshParams;
use amalthea::comm::variables_comm::UpdateParams;
use amalthea::comm::variables_comm::Variable;
use amalthea::comm::variables_comm::VariableList;
use amalthea::comm::variables_comm::VariablesBackendReply;
use amalthea::comm::variables_comm::VariablesBackendRequest;
use amalthea::comm::variables_comm::VariablesFrontendEvent;
use amalthea::socket::comm::CommSocket;
use crossbeam::channel::select;
use crossbeam::channel::unbounded;
use crossbeam::channel::Sender;
use harp::environment::Binding;
use harp::environment::Environment;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::object::RObject;
use harp::utils::r_assert_type;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use libR_shim::*;
use libr::R_GlobalEnv;
use log::debug;
use log::error;
use log::warn;
use stdext::spawn;

use crate::data_viewer::r_data_viewer::RDataViewer;
use crate::lsp::events::EVENTS;
use crate::r_task;
use crate::thread::RThreadSafe;
use crate::variables::variable::PositronVariable;

/**
 * The R Variables handler provides the server side of Positron's Variables panel, and is
 * responsible for creating and updating the list of variables.
 */
pub struct RVariables {
    comm: CommSocket,
    comm_manager_tx: Sender<CommManagerEvent>,
    pub env: RThreadSafe<RObject>,
    /// `Binding` does not currently protect anything, and therefore doesn't
    /// implement `Drop`, which might use the R API. It assumes that R SYMSXPs
    /// protect themselves, and that the binding value is protected by the
    /// `env`. This seems to work fine, so technically we don't need
    /// `RThreadSafe` to ensure that the `drop()` runs on the R main thread.
    /// However, we do need to `Send` the underlying `SEXP` values between
    /// threads, so we still use `RThreadSafe` for that.
    ///
    /// NOTE: What if the bindings get out of sync with the environment?
    /// Outside of R tasks, R will run concurrently with the environment
    /// thread and the bindings might be updated concurrently. There is a risk
    /// that the thread is then holding onto dangling pointers. For safety we
    /// should probably store the bindings in a list owned by the environment
    /// thread. Tracked in https://github.com/posit-dev/positron/issues/1812
    current_bindings: RThreadSafe<Vec<Binding>>,
    version: u64,
}

impl RVariables {
    /**
     * Creates a new RVariables instance.
     *
     * - `env`: An R environment to scan for variables, typically R_GlobalEnv
     * - `comm`: A channel used to send messages to the frontend
     */
    pub fn start(env: RObject, comm: CommSocket, comm_manager_tx: Sender<CommManagerEvent>) {
        // Validate that the RObject we were passed is actually an environment
        if let Err(err) = r_assert_type(env.sexp, &[ENVSXP]) {
            warn!(
                "Environment: Attempt to monitor or list non-environment object {:?} ({:?})",
                env, err
            );
        }

        // To be able to `Send` the `env` to the thread, it needs to be made
        // thread safe. To create `current_bindings`, we need to be on the main
        // R thread.
        let env = RThreadSafe::new(env);
        let current_bindings = RThreadSafe::new(vec![]);

        // Start the execution thread and wait for requests from the frontend
        spawn!("ark-variables", move || {
            // When `env` and `current_bindings` are dropped, a `r_async_task()`
            // call unprotects them
            let environment = Self {
                comm,
                comm_manager_tx,
                env,
                current_bindings,
                version: 0,
            };
            environment.execution_thread();
        });
    }

    pub fn execution_thread(mut self) {
        let (prompt_signal_tx, prompt_signal_rx) = unbounded::<()>();

        // Register a handler for console prompt events
        let listen_id = EVENTS.console_prompt.listen({
            move |_| {
                log::info!("Got console prompt signal.");
                prompt_signal_tx.send(()).unwrap();
            }
        });

        // Perform the initial environment scan and deliver to the frontend
        let variables = self.list_variables();
        let length = variables.len() as i64;
        let event = VariablesFrontendEvent::Refresh(RefreshParams {
            variables,
            length,
            version: self.version as i64,
        });
        self.send_event(event, None);

        // Flag initially set to false, but set to true if the user closes the
        // channel (i.e. the frontend is closed)
        let mut user_initiated_close = false;

        // Main message processing loop; we wait here for messages from the
        // frontend and loop as long as the channel is open
        loop {
            select! {
                recv(&prompt_signal_rx) -> msg => {
                    if let Ok(()) = msg {
                        self.update(None);
                    }
                },

                recv(&self.comm.incoming_rx) -> msg => {
                    let msg = match msg {
                        Ok(msg) => msg,
                        Err(e) => {
                            // We failed to receive a message from the frontend. This
                            // is usually not a transient issue and indicates that the
                            // channel is closed, so allowing the thread to exit is
                            // appropriate. Retrying is likely to just lead to a busy
                            // loop.
                            error!(
                                "Environment: Error receiving message from frontend: {:?}",
                                e
                            );

                            break;
                        },
                    };
                    debug!("Environment: Received message from frontend: {:?}", msg);

                    // Break out of the loop if the frontend has closed the channel
                    if let CommMsg::Close = msg {
                        debug!("Environment: Closing down after receiving comm_close from frontend.");

                        // Remember that the user initiated the close so that we can
                        // avoid sending a duplicate close message from the back end
                        user_initiated_close = true;
                        break;
                    }

                    let comm = self.comm.clone();
                    comm.handle_request(msg, |req| self.handle_rpc(req));
                }
            }
        }

        EVENTS.console_prompt.remove(listen_id);

        if !user_initiated_close {
            // Send a close message to the frontend if the frontend didn't
            // initiate the close
            self.comm.outgoing_tx.send(CommMsg::Close).unwrap();
        }
    }

    fn update_bindings(&mut self, new_bindings: RThreadSafe<Vec<Binding>>) -> u64 {
        // Updating will `drop()` the old `current_bindings` on the main R thread
        self.current_bindings = new_bindings;
        self.version = self.version + 1;

        self.version
    }

    fn list_variables(&mut self) -> Vec<Variable> {
        let mut variables: Vec<Variable> = vec![];

        r_task(|| {
            self.update_bindings(self.bindings());

            for binding in self.current_bindings.get() {
                variables.push(PositronVariable::new(binding).var());
            }
        });

        variables
    }

    fn handle_rpc(
        &mut self,
        req: VariablesBackendRequest,
    ) -> anyhow::Result<VariablesBackendReply> {
        match req {
            VariablesBackendRequest::List => {
                let list = self.list_variables();
                let count = list.len() as i64;
                Ok(VariablesBackendReply::ListReply(VariableList {
                    variables: list,
                    length: count,
                    version: Some(self.version as i64),
                }))
            },
            VariablesBackendRequest::Clear(params) => {
                self.clear(params.include_hidden_objects)?;
                self.update(None);
                Ok(VariablesBackendReply::ClearReply())
            },
            VariablesBackendRequest::Delete(params) => {
                self.delete(params.names.clone())?;
                Ok(VariablesBackendReply::DeleteReply(params.names))
            },
            VariablesBackendRequest::Inspect(params) => {
                let children = self.inspect(&params.path)?;
                let count = children.len() as i64;
                Ok(VariablesBackendReply::InspectReply(InspectedVariable {
                    children,
                    length: count,
                }))
            },
            VariablesBackendRequest::ClipboardFormat(params) => {
                let content = self.clipboard_format(&params.path, params.format.clone())?;
                Ok(VariablesBackendReply::ClipboardFormatReply(
                    FormattedVariable { content },
                ))
            },
            VariablesBackendRequest::View(params) => {
                self.view(&params.path)?;
                Ok(VariablesBackendReply::ViewReply())
            },
        }
    }

    /**
     * Clear the environment. Uses rm(envir = <env>, list = ls(<env>, all.names = TRUE))
     */
    fn clear(&mut self, include_hidden_objects: bool) -> Result<(), harp::error::Error> {
        r_task(|| unsafe {
            let env = self.env.get().clone();

            let mut list = RFunction::new("base", "ls")
                .param("envir", *env)
                .param("all.names", Rf_ScalarLogical(include_hidden_objects as i32))
                .call()?;

            if *env == R_GlobalEnv {
                list = RFunction::new("base", "setdiff")
                    .add(list)
                    .add(RObject::from(".Random.seed"))
                    .call()?;
            }

            RFunction::new("base", "rm")
                .param("list", list)
                .param("envir", *env)
                .call()?;

            Ok(())
        })
    }

    /**
     * Clear the environment. Uses rm(envir = <env>, list = ls(<env>, all.names = TRUE))
     */
    fn delete(&mut self, variables: Vec<String>) -> Result<(), harp::error::Error> {
        r_task(|| unsafe {
            let variables: Vec<&str> = variables.iter().map(|s| s as &str).collect();

            let env = self.env.get().clone();

            let result = RFunction::new("base", "rm")
                .param("list", CharacterVector::create(variables).cast())
                .param("envir", env)
                .call();

            if let Err(err) = result {
                return Err(err);
            }
            Ok(())
        })
    }

    fn clipboard_format(
        &mut self,
        path: &Vec<String>,
        format: ClipboardFormatFormat,
    ) -> Result<String, harp::error::Error> {
        r_task(|| {
            let env = self.env.get().clone();
            PositronVariable::clip(env, &path, &format)
        })
    }

    fn inspect(&mut self, path: &Vec<String>) -> Result<Vec<Variable>, harp::error::Error> {
        r_task(|| {
            let env = self.env.get().clone();
            PositronVariable::inspect(env, &path)
        })
    }

    /// Open a data viewer for the given variable.
    ///
    /// - `path`: The path to the variable to view, as an array of access keys
    fn view(&mut self, path: &Vec<String>) -> Result<(), harp::error::Error> {
        r_task(|| {
            let env = self.env.get().clone();
            let data = PositronVariable::resolve_data_object(env, &path)?;
            let name = unsafe { path.get_unchecked(path.len() - 1) };
            RDataViewer::start(name.clone(), data, self.comm_manager_tx.clone());
            Ok(())
        })
    }

    fn send_event(&mut self, message: VariablesFrontendEvent, request_id: Option<String>) {
        let data = serde_json::to_value(message);

        match data {
            Ok(data) => {
                // If we were given a request ID, send the response as an RPC;
                // otherwise, send it as an event
                let comm_msg = match request_id {
                    Some(id) => CommMsg::Rpc(id, data),
                    None => CommMsg::Data(data),
                };

                self.comm.outgoing_tx.send(comm_msg).unwrap()
            },
            Err(err) => {
                error!("Environment: Failed to serialize environment data: {}", err);
            },
        }
    }

    fn update(&mut self, request_id: Option<String>) {
        let mut assigned: Vec<Variable> = vec![];
        let mut removed: Vec<String> = vec![];

        r_task(|| {
            let new_bindings = self.bindings();

            let mut old_iter = self.current_bindings.get().iter();
            let mut old_next = old_iter.next();

            let mut new_iter = new_bindings.get().iter();
            let mut new_next = new_iter.next();

            loop {
                match (old_next, new_next) {
                    // nothing more to do
                    (None, None) => break,

                    // No more old, collect last new into added
                    (None, Some(mut new)) => {
                        loop {
                            assigned.push(PositronVariable::new(&new).var());

                            match new_iter.next() {
                                Some(x) => {
                                    new = x;
                                },
                                None => break,
                            };
                        }
                        break;
                    },

                    // No more new, collect the last old into removed
                    (Some(mut old), None) => {
                        loop {
                            removed.push(old.name.to_string());

                            match old_iter.next() {
                                Some(x) => {
                                    old = x;
                                },
                                None => break,
                            };
                        }

                        break;
                    },

                    (Some(old), Some(new)) => {
                        if old.name == new.name {
                            if old.value != new.value {
                                assigned.push(PositronVariable::new(&new).var());
                            }
                            old_next = old_iter.next();
                            new_next = new_iter.next();
                        } else if old.name < new.name {
                            removed.push(old.name.to_string());
                            old_next = old_iter.next();
                        } else {
                            assigned.push(PositronVariable::new(&new).var());
                            new_next = new_iter.next();
                        }
                    },
                }
            }

            // Only update the bindings (and the version) if anything changed
            if assigned.len() > 0 || removed.len() > 0 {
                self.update_bindings(new_bindings);
            }
        });

        if assigned.len() > 0 || removed.len() > 0 || request_id.is_some() {
            // Send the message if anything changed or if this came from a request
            let event = VariablesFrontendEvent::Update(UpdateParams {
                assigned,
                removed,
                version: self.version as i64,
            });
            self.send_event(event, request_id);
        }
    }

    // SAFETY: The following methods must be called in an `r_task()`

    fn bindings(&self) -> RThreadSafe<Vec<Binding>> {
        let env = self.env.get().clone();
        let env = Environment::new(env);
        let mut bindings: Vec<Binding> =
            env.iter().filter(|binding| !binding.is_hidden()).collect();
        bindings.sort_by(|a, b| a.name.cmp(&b.name));
        let bindings = RThreadSafe::new(bindings);
        bindings
    }
}
