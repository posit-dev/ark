//
// r_variables.rs
//
// Copyright (C) 2023 by Posit Software, PBC
//
//

use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::event::CommManagerEvent;
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
use log::debug;
use log::error;
use log::warn;
use stdext::spawn;

use crate::data_viewer::r_data_viewer::RDataViewer;
use crate::lsp::events::EVENTS;
use crate::r_task;
use crate::thread::RThreadSafe;
use crate::variables::message::VariablesMessage;
use crate::variables::message::VariablesMessageClear;
use crate::variables::message::VariablesMessageClipboardFormat;
use crate::variables::message::VariablesMessageDelete;
use crate::variables::message::VariablesMessageDetails;
use crate::variables::message::VariablesMessageError;
use crate::variables::message::VariablesMessageFormattedVariable;
use crate::variables::message::VariablesMessageInspect;
use crate::variables::message::VariablesMessageList;
use crate::variables::message::VariablesMessageUpdate;
use crate::variables::message::VariablesMessageView;
use crate::variables::variable::Variable;

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
     * - `comm`: A channel used to send messages to the front end
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

        // Start the execution thread and wait for requests from the front end
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

        // Perform the initial environment scan and deliver to the front end
        self.refresh(None);

        // Flag initially set to false, but set to true if the user closes the
        // channel (i.e. the front end is closed)
        let mut user_initiated_close = false;

        // Main message processing loop; we wait here for messages from the
        // front end and loop as long as the channel is open
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
                            // We failed to receive a message from the front end. This
                            // is usually not a transient issue and indicates that the
                            // channel is closed, so allowing the thread to exit is
                            // appropriate. Retrying is likely to just lead to a busy
                            // loop.
                            error!(
                                "Environment: Error receiving message from front end: {:?}",
                                e
                            );

                            break;
                        },
                    };
                    debug!("Environment: Received message from front end: {:?}", msg);

                    // Break out of the loop if the front end has closed the channel
                    if msg == CommMsg::Close {
                        debug!("Environment: Closing down after receiving comm_close from front end.");

                        // Remember that the user initiated the close so that we can
                        // avoid sending a duplicate close message from the back end
                        user_initiated_close = true;
                        break;
                    }

                    // Process ordinary data messages
                    if let CommMsg::Rpc(id, data) = msg {
                        let message = match serde_json::from_value::<VariablesMessage>(data) {
                            Ok(m) => m,
                            Err(err) => {
                                error!(
                                    "Environment: Received invalid message from front end. {}",
                                    err
                                );
                                continue;
                            },
                        };

                        // Match on the type of data received.
                        match message {
                            // This is a request to refresh the variables list, so perform a full
                            // environment scan and deliver to the front end
                            VariablesMessage::Refresh => {
                                self.refresh(Some(id));
                            },

                            VariablesMessage::Clear(VariablesMessageClear{include_hidden_objects}) => {
                                self.clear(include_hidden_objects, Some(id));
                            },

                            VariablesMessage::Delete(VariablesMessageDelete{variables}) => {
                                self.delete(variables, Some(id));
                            },

                            VariablesMessage::Inspect(VariablesMessageInspect{path}) => {
                                self.inspect(&path, Some(id));
                            },

                            VariablesMessage::ClipboardFormat(VariablesMessageClipboardFormat{path, format}) => {
                                self.clipboard_format(&path, format, Some(id));
                            },

                            VariablesMessage::View(VariablesMessageView { path }) => {
                                self.view(&path, Some(id));
                            },

                            _ => {
                                error!(
                                    "Environment: Don't know how to handle message type '{:?}'",
                                    message
                                );
                            },
                        }
                    }
                }
            }
        }

        EVENTS.console_prompt.remove(listen_id);

        if !user_initiated_close {
            // Send a close message to the front end if the front end didn't
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

    /**
     * Perform a full environment scan and deliver the results to the front end.
     * When this message is being sent in reply to a request from the front end,
     * the request ID is passed in as an argument.
     */
    fn refresh(&mut self, request_id: Option<String>) {
        let mut variables: Vec<Variable> = vec![];

        r_task(|| {
            self.update_bindings(self.bindings());

            for binding in self.current_bindings.get() {
                variables.push(Variable::new(binding));
            }
        });

        // TODO: Avoid serializing the full list of variables if it exceeds a
        // certain threshold
        let env_size = variables.len();
        let env_list = VariablesMessage::List(VariablesMessageList {
            variables,
            length: env_size,
            version: self.version,
        });

        self.send_message(env_list, request_id);
    }

    /**
     * Clear the environment. Uses rm(envir = <env>, list = ls(<env>, all.names = TRUE))
     */
    fn clear(&mut self, include_hidden_objects: bool, request_id: Option<String>) {
        // try to rm(<env>, list = ls(envir = <env>, all.names = TRUE)))
        let result: Result<(), harp::error::Error> = r_task(|| unsafe {
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
        });

        if let Err(_err) = result {
            error!("Failed to clear the environment");
        }

        // and then refresh anyway
        //
        // it is possible (is it ?) that in case of an error some variables
        // were removed and some were not
        self.refresh(request_id);
    }

    /**
     * Clear the environment. Uses rm(envir = <env>, list = ls(<env>, all.names = TRUE))
     */
    fn delete(&mut self, variables: Vec<String>, request_id: Option<String>) {
        r_task(|| unsafe {
            let variables: Vec<&str> = variables.iter().map(|s| s as &str).collect();

            let env = self.env.get().clone();

            let result = RFunction::new("base", "rm")
                .param("list", CharacterVector::create(variables).cast())
                .param("envir", env)
                .call();

            if let Err(_) = result {
                error!("Failed to delete variables from the environment");
            }
        });

        // and then update
        self.update(request_id);
    }

    fn clipboard_format(&mut self, path: &Vec<String>, format: String, request_id: Option<String>) {
        let clipped = r_task(|| {
            let env = self.env.get().clone();
            Variable::clip(env, &path, &format)
        });

        let msg = match clipped {
            Ok(content) => VariablesMessage::FormattedVariable(VariablesMessageFormattedVariable {
                format,
                content,
            }),

            Err(_) => VariablesMessage::Error(VariablesMessageError {
                message: String::from("Clipboard Format error"),
            }),
        };
        self.send_message(msg, request_id);
    }

    fn inspect(&mut self, path: &Vec<String>, request_id: Option<String>) {
        let inspect = r_task(|| {
            let env = self.env.get().clone();
            Variable::inspect(env, &path)
        });
        let msg = match inspect {
            Ok(children) => {
                let length = children.len();
                VariablesMessage::Details(VariablesMessageDetails {
                    path: path.clone(),
                    children,
                    length,
                })
            },
            Err(_) => VariablesMessage::Error(VariablesMessageError {
                message: String::from("Inspection error"),
            }),
        };

        self.send_message(msg, request_id);
    }

    fn view(&mut self, path: &Vec<String>, request_id: Option<String>) {
        let message = r_task(|| {
            let env = self.env.get().clone();

            let data = Variable::resolve_data_object(env, &path);

            match data {
                Ok(data) => {
                    let name = unsafe { path.get_unchecked(path.len() - 1) };
                    RDataViewer::start(name.clone(), data, self.comm_manager_tx.clone());
                    VariablesMessage::Success
                },

                Err(_) => VariablesMessage::Error(VariablesMessageError {
                    message: String::from("Inspection error"),
                }),
            }
        });

        self.send_message(message, request_id);
    }

    fn send_message(&mut self, message: VariablesMessage, request_id: Option<String>) {
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
                            assigned.push(Variable::new(&new));

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
                                assigned.push(Variable::new(&new));
                            }
                            old_next = old_iter.next();
                            new_next = new_iter.next();
                        } else if old.name < new.name {
                            removed.push(old.name.to_string());
                            old_next = old_iter.next();
                        } else {
                            assigned.push(Variable::new(&new));
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
            let message = VariablesMessage::Update(VariablesMessageUpdate {
                assigned,
                removed,
                version: self.version,
            });
            self.send_message(message, request_id);
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
