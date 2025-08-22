//
// r_variables.rs
//
// Copyright (C) 2023-2025 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
use amalthea::comm::variables_comm::ClipboardFormatFormat;
use amalthea::comm::variables_comm::FormattedVariable;
use amalthea::comm::variables_comm::InspectedVariable;
use amalthea::comm::variables_comm::QueryTableSummaryResult;
use amalthea::comm::variables_comm::RefreshParams;
use amalthea::comm::variables_comm::UpdateParams;
use amalthea::comm::variables_comm::Variable;
use amalthea::comm::variables_comm::VariableList;
use amalthea::comm::variables_comm::VariablesBackendReply;
use amalthea::comm::variables_comm::VariablesBackendRequest;
use amalthea::comm::variables_comm::VariablesFrontendEvent;
use amalthea::socket::comm::CommSocket;
use anyhow::anyhow;
use crossbeam::channel::select;
use crossbeam::channel::unbounded;
use crossbeam::channel::Sender;
use harp::environment::Binding;
use harp::environment::Environment;
use harp::environment::EnvironmentFilter;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::get_option;
use harp::object::RObject;
use harp::utils::r_assert_type;
use harp::utils::r_is_function;
use harp::vector::CharacterVector;
use harp::vector::Vector;
use libr::R_GlobalEnv;
use libr::Rf_ScalarLogical;
use libr::ENVSXP;
use stdext::spawn;

use crate::data_explorer::r_data_explorer::DataObjectEnvInfo;
use crate::data_explorer::r_data_explorer::RDataExplorer;
use crate::data_explorer::summary_stats::summary_stats;
use crate::lsp::events::EVENTS;
use crate::r_task;
use crate::thread::RThreadSafe;
use crate::variables::variable::PositronVariable;
use crate::view::view;

/// Enumeration of treatments for the .Last.value variable
pub enum LastValue {
    /// Always show the .Last.value variable in the Variables pane. This is used
    /// by tests to show the value without changing the global option.
    Always,

    /// Use the value of the global option `positron.show_last_value` to
    /// determine whether to show the .Last.value variable
    UseOption,
}

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

    /// Whether to always show the .Last.value in the Variables pane, regardless
    /// of the value of positron.show_last_value
    show_last_value: LastValue,

    /// Whether we are currently showing the .Last.value variable in the Variables
    /// pane.
    showing_last_value: bool,
}

impl RVariables {
    /**
     * Creates a new RVariables instance.
     *
     * - `env`: An R environment to scan for variables, typically R_GlobalEnv
     * - `comm`: A channel used to send messages to the frontend
     */
    pub fn start(env: RObject, comm: CommSocket, comm_manager_tx: Sender<CommManagerEvent>) {
        // Start with default settings
        Self::start_with_config(env, comm, comm_manager_tx, LastValue::UseOption);
    }

    /**
     * Creates a new RVariables instance with specific configuration.
     *
     * - `env`: An R environment to scan for variables, typically R_GlobalEnv
     * - `comm`: A channel used to send messages to the frontend
     * - `show_last_value`: Whether to include .Last.value in the variables list
     */
    pub fn start_with_config(
        env: RObject,
        comm: CommSocket,
        comm_manager_tx: Sender<CommManagerEvent>,
        show_last_value: LastValue,
    ) {
        // Validate that the RObject we were passed is actually an environment
        if let Err(err) = r_assert_type(env.sexp, &[ENVSXP]) {
            log::warn!(
                "Variables: Attempt to monitor or list non-environment object {env:?} ({err:?})"
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
                show_last_value,
                showing_last_value: false,
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
                        Err(err) => {
                            // We failed to receive a message from the frontend. This
                            // is usually not a transient issue and indicates that the
                            // channel is closed, so allowing the thread to exit is
                            // appropriate. Retrying is likely to just lead to a busy
                            // loop.
                            log::error!(
                                "Variables: Error receiving message from frontend: {err:?}"
                            );

                            break;
                        },
                    };
                    log::info!("Variables: Received message from frontend: {msg:?}");

                    // Break out of the loop if the frontend has closed the channel
                    if let CommMsg::Close = msg {
                        log::info!("Variables: Closing down after receiving comm_close from frontend.");

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

    #[tracing::instrument(level = "trace", skip_all)]
    fn list_variables(&mut self) -> Vec<Variable> {
        let mut variables: Vec<Variable> = vec![];
        r_task(|| {
            self.update_bindings(self.bindings());

            // If the special .Last.value variable is enabled, add it to the
            // list. This is a special R value that doesn't have its own
            // binding.
            if let Some(last_value) = self.last_value() {
                self.showing_last_value = true;
                variables.push(last_value.var());
            }

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
                let viewer_id = self.view(&params.path)?;
                Ok(VariablesBackendReply::ViewReply(viewer_id))
            },
            VariablesBackendRequest::QueryTableSummary(params) => {
                let result = self.query_table_summary(&params.path, &params.query_types)?;
                Ok(VariablesBackendReply::QueryTableSummaryReply(result))
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
        r_task(|| {
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
    ) -> anyhow::Result<String> {
        r_task(|| {
            let env = self.env.get().clone();
            PositronVariable::clip(env, &path, &format)
        })
    }

    fn inspect(&mut self, path: &Vec<String>) -> anyhow::Result<Vec<Variable>> {
        r_task(|| {
            let env = self.env.get().clone();
            PositronVariable::inspect(env, &path)
        })
    }

    /// Open a data viewer for the given variable.
    ///
    /// - `path`: The path to the variable to view, as an array of access keys
    ///
    /// Returns the ID of the comm managing the view, if any.
    fn view(&mut self, path: &Vec<String>) -> Result<Option<String>, harp::error::Error> {
        r_task(|| {
            let env = self.env.get().clone();
            let obj = PositronVariable::resolve_data_object(env.clone(), &path)?;

            if r_is_function(obj.sexp) {
                harp::as_result(view(&obj, &path, &env))?;
                return Ok(None);
            }

            let name = unsafe { path.get_unchecked(path.len() - 1) };

            let binding = DataObjectEnvInfo {
                name: name.to_string(),
                env: RThreadSafe::new(env),
            };

            let viewer_id = RDataExplorer::start(
                name.clone(),
                obj,
                Some(binding),
                self.comm_manager_tx.clone(),
            )?;
            Ok(Some(viewer_id))
        })
    }

    /// Query table summary for the given variable.
    ///
    /// - `path`: The path to the variable to summarize, as an array of access keys
    /// - `query_types`: A list of query types (e.g. "summary_stats")
    ///
    /// Returns summary information about the table including schemas and profiles.
    fn query_table_summary(
        &mut self,
        path: &Vec<String>,
        query_types: &Vec<String>,
    ) -> anyhow::Result<QueryTableSummaryResult> {
        r_task(|| {
            let env = self.env.get().clone();
            let table = PositronVariable::resolve_data_object(env, &path)?;

            let kind = if harp::utils::r_is_data_frame(table.sexp) {
                harp::TableKind::Dataframe
            } else if harp::utils::r_is_matrix(table.sexp) {
                harp::TableKind::Matrix
            } else {
                return Err(anyhow!(
                    "Object is not a supported table type (data.frame or matrix)"
                ));
            };

            let num_cols = match kind {
                harp::TableKind::Dataframe => {
                    let ncol = harp::DataFrame::n_col(table.sexp)?;
                    ncol as i64
                },
                harp::TableKind::Matrix => {
                    let (_nrow, ncol) = harp::Matrix::dim(table.sexp)?;
                    ncol as i64
                },
            };

            let shapes = RDataExplorer::r_get_shape(table.clone())?;

            let column_schemas: Vec<String> = shapes
                .columns
                .iter()
                .map(|schema| serde_json::to_string(schema))
                .collect::<Result<Vec<_>, _>>()?;

            let mut column_profiles: Vec<String> = vec![];

            if query_types.contains(&"summary_stats".to_string()) {
                let profiles: Vec<String> = shapes
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, schema)| -> anyhow::Result<String> {
                        let column = harp::tbl_get_column(table.sexp, i as i32, kind)?;

                        let format_options = amalthea::comm::data_explorer_comm::FormatOptions {
                            large_num_digits: 4,
                            small_num_digits: 6,
                            max_integral_digits: 7,
                            max_value_length: 1000,
                            thousands_sep: None,
                        };

                        let summary_stats =
                            summary_stats(column.sexp, schema.type_display, &format_options).map(
                                |stats| {
                                    serde_json::to_value(stats).unwrap_or(serde_json::Value::Null)
                                },
                            )?;

                        let profile = serde_json::json!({
                            "column_name": schema.column_name,
                            "type_display": format!("{:?}", schema.type_display).to_lowercase(),
                            "summary_stats": summary_stats,
                        })
                        .to_string();

                        Ok(profile)
                    })
                    .collect::<anyhow::Result<Vec<String>>>()?;

                column_profiles.extend(profiles);
            }

            Ok(QueryTableSummaryResult {
                num_rows: shapes.num_rows as i64,
                num_columns: num_cols,
                column_schemas,
                column_profiles,
            })
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
                log::error!("Variables: Failed to serialize environment data: {err}");
            },
        }
    }

    /// Gets the value of the special variable '.Last.value' (the value of the
    /// last expression evaluated at the top level), if enabled.
    ///
    /// Returns None in all other cases.
    fn last_value(&self) -> Option<PositronVariable> {
        // Check the cached value first
        let show_last_value = match self.show_last_value {
            LastValue::Always => true,
            LastValue::UseOption => {
                // If we aren't always showing the last value, update from the
                // global option
                let use_last_value = get_option("positron.show_last_value");
                match use_last_value.get_bool(0) {
                    Ok(Some(true)) => true,
                    _ => false,
                }
            },
        };

        if show_last_value {
            match harp::environment::last_value() {
                Ok(last_robj) => Some(PositronVariable::from(
                    String::from(".Last.value"),
                    String::from(".Last.value"),
                    last_robj.sexp,
                )),
                Err(err) => {
                    // This isn't a critical error but would also be very
                    // unexpected.
                    log::error!("Variables: Could not evaluate .Last.value ({err:?})");
                    None
                },
            }
        } else {
            // Last value display is disabled
            None
        }
    }

    #[tracing::instrument(level = "trace", skip_all)]
    fn update(&mut self, request_id: Option<String>) {
        let mut assigned: Vec<Variable> = vec![];
        let mut removed: Vec<String> = vec![];

        r_task(|| {
            let new_bindings = self.bindings();

            let mut old_iter = self.current_bindings.get().iter();
            let mut old_next = old_iter.next();

            let mut new_iter = new_bindings.get().iter();
            let mut new_next = new_iter.next();

            // Track the last value if the user has requested it. Treat this
            // value as assigned every time we update the Variables list.
            if let Some(last_value) = self.last_value() {
                self.showing_last_value = true;
                assigned.push(last_value.var());
            } else if self.showing_last_value {
                // If we are no longer showing the last value, remove it from
                // the list of assigned variables
                self.showing_last_value = false;
                removed.push(".Last.value".to_string());
            }

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
                            if old.value.id() != new.value.id() {
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
                unevaluated: vec![],
                version: self.version as i64,
            });
            self.send_event(event, request_id);
        }
    }

    // SAFETY: The following methods must be called in an `r_task()`

    fn bindings(&self) -> RThreadSafe<Vec<Binding>> {
        let env = self.env.get().clone();
        let env = Environment::new_filtered(env, EnvironmentFilter::ExcludeHidden);

        let mut bindings: Vec<Binding> = env.iter().filter_map(|b| b.ok()).collect();

        bindings.sort_by(|a, b| a.name.cmp(&b.name));

        RThreadSafe::new(bindings)
    }
}
