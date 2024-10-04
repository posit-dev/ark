//
// interface.rs
//
// Copyright (C) 2023-2024 Posit Software, PBC. All rights reserved.
//
//

// All code in this file runs synchronously with R. We store the global
// state inside of a global `R_MAIN` singleton that implements `RMain`.
// The frontend methods called by R are forwarded to the corresponding
// `RMain` methods via `R_MAIN`.

use std::collections::HashMap;
use std::ffi::*;
use std::os::raw::c_uchar;
use std::path::PathBuf;
use std::result::Result::Ok;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::Poll;
use std::time::Duration;

use amalthea::comm::base_comm::JsonRpcReply;
use amalthea::comm::event::CommManagerEvent;
use amalthea::comm::ui_comm::ui_frontend_reply_from_value;
use amalthea::comm::ui_comm::BusyParams;
use amalthea::comm::ui_comm::PromptStateParams;
use amalthea::comm::ui_comm::ShowMessageParams;
use amalthea::comm::ui_comm::UiFrontendEvent;
use amalthea::comm::ui_comm::UiFrontendRequest;
use amalthea::socket::iopub::IOPubMessage;
use amalthea::socket::iopub::Wait;
use amalthea::socket::stdin::StdInRequest;
use amalthea::wire::exception::Exception;
use amalthea::wire::execute_error::ExecuteError;
use amalthea::wire::execute_input::ExecuteInput;
use amalthea::wire::execute_reply::ExecuteReply;
use amalthea::wire::execute_reply_exception::ExecuteReplyException;
use amalthea::wire::execute_request::ExecuteRequest;
use amalthea::wire::execute_response::ExecuteResponse;
use amalthea::wire::execute_result::ExecuteResult;
use amalthea::wire::input_reply::InputReply;
use amalthea::wire::input_request::InputRequest;
use amalthea::wire::input_request::ShellInputRequest;
use amalthea::wire::input_request::StdInRpcReply;
use amalthea::wire::input_request::UiCommFrontendRequest;
use amalthea::wire::jupyter_message::Status;
use amalthea::wire::originator::Originator;
use amalthea::wire::stream::Stream;
use amalthea::wire::stream::StreamOutput;
use amalthea::Error;
use anyhow::*;
use bus::Bus;
use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use harp::environment::r_ns_env;
use harp::environment::Environment;
use harp::environment::R_ENVS;
use harp::exec::r_check_stack;
use harp::exec::r_peek_error_buffer;
use harp::exec::r_sandbox;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::library::RLibraries;
use harp::line_ending::convert_line_endings;
use harp::line_ending::LineEnding;
use harp::object::r_null_or_try_into;
use harp::object::RObject;
use harp::r_symbol;
use harp::routines::r_register_routines;
use harp::session::r_traceback;
use harp::utils::r_is_data_frame;
use harp::utils::r_typeof;
use harp::R_MAIN_THREAD_ID;
use libr::R_BaseNamespace;
use libr::R_GlobalEnv;
use libr::R_ProcessEvents;
use libr::R_RunPendingFinalizers;
use libr::Rf_error;
use libr::Rf_findVarInFrame;
use libr::Rf_onintr;
use libr::SEXP;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::json;
use stdext::result::ResultOrLog;
use stdext::*;
use uuid::Uuid;

use crate::dap::dap::DapBackendEvent;
use crate::dap::dap_r_main::RMainDap;
use crate::dap::Dap;
use crate::errors;
use crate::help::message::HelpEvent;
use crate::help::r_help::RHelp;
use crate::kernel::Kernel;
use crate::lsp::events::EVENTS;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::KernelNotification;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::lsp::state_handlers::ConsoleInputs;
use crate::modules;
use crate::plots::graphics_device;
use crate::r_task;
use crate::r_task::BoxFuture;
use crate::r_task::RTask;
use crate::r_task::RTaskStartInfo;
use crate::r_task::RTaskStatus;
use crate::request::debug_request_command;
use crate::request::RRequest;
use crate::signals::initialize_signal_handlers;
use crate::signals::interrupts_pending;
use crate::signals::set_interrupts_pending;
use crate::srcref::ns_populate_srcref;
use crate::srcref::resource_loaded_namespaces;
use crate::startup;
use crate::strings::lines;
use crate::sys::console::console_to_utf8;

static RE_DEBUG_PROMPT: Lazy<Regex> = Lazy::new(|| Regex::new(r"Browse\[\d+\]").unwrap());

/// An enum representing the different modes in which the R session can run.
#[derive(PartialEq, Clone)]
pub enum SessionMode {
    /// A session with an interactive console (REPL), such as in Positron.
    Console,

    /// A session in a Jupyter or Jupyter-like notebook.
    Notebook,

    /// A background session, typically not connected to any UI.
    Background,
}

// --- Globals ---
// These values must be global in order for them to be accessible from R
// callbacks, which do not have a facility for passing or returning context.

// We use the `once_cell` crate for init synchronisation because the stdlib
// equivalent `std::sync::Once` does not have a `wait()` method.

/// Used to wait for complete R startup in `RMain::wait_initialized()` or
/// check for it in `RMain::is_initialized()`.
static R_INIT: once_cell::sync::OnceCell<()> = once_cell::sync::OnceCell::new();

// The global state used by R callbacks.
//
// Doesn't need a mutex because it's only accessed by the R thread. Should
// not be used elsewhere than from an R frontend callback or an R function
// invoked by the REPL (this is enforced by `RMain::get()` and
// `RMain::get_mut()`).
static mut R_MAIN: Option<RMain> = None;

/// Banner output accumulated during startup
static mut R_BANNER: String = String::new();

pub struct RMain {
    kernel_init_tx: Bus<KernelInfo>,

    /// Whether we are running in Console, Notebook, or Background mode.
    pub session_mode: SessionMode,

    /// Channel used to send along messages relayed on the open comms.
    comm_manager_tx: Sender<CommManagerEvent>,

    /// Execution requests from the frontend. Processed from `ReadConsole()`.
    /// Requests for code execution provide input to that method.
    r_request_rx: Receiver<RRequest>,

    /// Input requests to the frontend. Processed from `ReadConsole()`
    /// calls triggered by e.g. `readline()`.
    stdin_request_tx: Sender<StdInRequest>,

    /// Input replies from the frontend. Waited on in `ReadConsole()` after a request.
    stdin_reply_rx: Receiver<amalthea::Result<InputReply>>,

    /// IOPub channel for broadcasting outputs
    iopub_tx: Sender<IOPubMessage>,

    /// Active request passed to `ReadConsole()`. Contains response channel
    /// the reply should be send to once computation has finished.
    active_request: Option<ActiveReadConsoleRequest>,

    /// Execution request counter used to populate `In[n]` and `Out[n]` prompts
    execution_count: u32,

    /// Accumulated top-level output for the current execution.
    /// This is the output emitted by R's autoprint and propagated as
    /// `execute_result` Jupyter messages instead of `stream` messages.
    autoprint_output: String,

    /// Channel to send and receive tasks from `RTask`s
    tasks_interrupt_rx: Receiver<RTask>,
    tasks_idle_rx: Receiver<RTask>,
    pending_futures: HashMap<Uuid, (BoxFuture<'static, ()>, RTaskStartInfo)>,

    /// Shared reference to kernel. Currently used by the ark-execution
    /// thread, the R frontend callbacks, and LSP routines called from R
    kernel: Arc<Mutex<Kernel>>,

    /// Represents whether an error occurred during R code execution.
    pub error_occurred: bool,
    pub error_message: String, // `evalue` in the Jupyter protocol
    pub error_traceback: Vec<String>,

    /// Channel to communicate with the Help thread
    help_event_tx: Option<Sender<HelpEvent>>,
    /// R help port
    help_port: Option<u16>,

    /// Event channel for notifying the LSP. In principle, could be a Jupyter comm.
    lsp_events_tx: Option<TokioUnboundedSender<Event>>,

    dap: RMainDap,

    /// Whether or not R itself is actively busy.
    /// This does not represent the busy state of the kernel.
    pub is_busy: bool,

    pub positron_ns: Option<RObject>,

    pending_lines: Vec<String>,
}

/// Represents the currently active execution request from the frontend. It
/// resolves at the next invocation of the `ReadConsole()` frontend method.
struct ActiveReadConsoleRequest {
    exec_count: u32,
    request: ExecuteRequest,
    orig: Option<Originator>,
    response_tx: Sender<ExecuteResponse>,
}

/// Represents kernel metadata (available after the kernel has fully started)
#[derive(Debug, Clone)]
pub struct KernelInfo {
    pub version: String,
    pub banner: String,
    pub input_prompt: Option<String>,
    pub continuation_prompt: Option<String>,
}

/// This struct represents the data that we wish R would pass to
/// `ReadConsole()` methods. We need this information to determine what kind
/// of prompt we are dealing with.
#[derive(Clone)]
pub struct PromptInfo {
    /// The prompt string to be presented to the user. This does not
    /// necessarily correspond to `getOption("prompt")`, for instance in
    /// case of a browser prompt or a readline prompt.
    input_prompt: String,

    /// The continuation prompt string when user supplies incomplete
    /// inputs. This always corresponds to `getOption("continue"). We send
    /// it to frontends along with `prompt` because some frontends such as
    /// Positron do not send incomplete inputs to Ark and take charge of
    /// continuation prompts themselves. For frontends that can send
    /// incomplete inputs to Ark, like Jupyter Notebooks, we immediately
    /// error on them rather than requesting that this be shown.
    continuation_prompt: String,

    /// Whether this is a `browser()` prompt. A browser prompt can be
    /// incomplete but is never a user request.
    browser: bool,

    /// Whether the last input didn't fully parse and R is waiting for more input
    incomplete: bool,

    /// Whether this is a prompt from a fresh REPL iteration (browser or
    /// top level) or a prompt from some user code, e.g. via `readline()`
    input_request: bool,
}

pub enum ConsoleInput {
    EOF,
    Input(String),
}

pub enum ConsoleResult {
    NewInput,
    Interrupt,
    Disconnected,
    Error(amalthea::Error),
}

impl RMain {
    /// Sets up the main R thread and initializes the `R_MAIN` singleton. Must
    /// be called only once. This is doing as much setup as possible before
    /// starting the R REPL. Since the REPL does not return, it might be
    /// launched in a background thread (which we do in integration tests). The
    /// setup can still be done in your main thread so that panics during setup
    /// may propagate as expected. Call `RMain::start()` after this to actually
    /// start the R REPL.
    pub fn setup(
        r_args: Vec<String>,
        startup_file: Option<String>,
        kernel_mutex: Arc<Mutex<Kernel>>,
        comm_manager_tx: Sender<CommManagerEvent>,
        r_request_rx: Receiver<RRequest>,
        stdin_request_tx: Sender<StdInRequest>,
        stdin_reply_rx: Receiver<amalthea::Result<InputReply>>,
        iopub_tx: Sender<IOPubMessage>,
        kernel_init_tx: Bus<KernelInfo>,
        dap: Arc<Mutex<Dap>>,
        session_mode: SessionMode,
    ) {
        // Channels to send/receive tasks from auxiliary threads via `RTask`s
        let (tasks_interrupt_tx, tasks_interrupt_rx) = unbounded::<RTask>();
        let (tasks_idle_tx, tasks_idle_rx) = unbounded::<RTask>();

        r_task::initialize(tasks_interrupt_tx.clone(), tasks_idle_tx.clone());

        unsafe {
            R_MAIN = Some(RMain::new(
                kernel_mutex,
                tasks_interrupt_rx,
                tasks_idle_rx,
                comm_manager_tx,
                r_request_rx,
                stdin_request_tx,
                stdin_reply_rx,
                iopub_tx,
                kernel_init_tx,
                dap,
                session_mode,
            ));
        };
        let r_main = unsafe { R_MAIN.as_mut().unwrap() };

        let mut r_args = r_args.clone();

        // Record if the user has requested that we don't load the site/user level R profiles
        let ignore_site_r_profile = startup::should_ignore_site_r_profile(&r_args);
        let ignore_user_r_profile = startup::should_ignore_user_r_profile(&r_args);

        // We always manually load site/user level R profiles rather than letting R do it
        // to ensure that ark is fully set up before running code that could potentially call
        // back into ark internals.
        if !ignore_site_r_profile {
            startup::push_ignore_site_r_profile(&mut r_args);
        }
        if !ignore_user_r_profile {
            startup::push_ignore_user_r_profile(&mut r_args);
        }

        // Build the argument list from the command line arguments. The default
        // list is `--interactive` unless altered with the `--` passthrough
        // argument.
        let mut args = cargs!["ark"];
        for arg in r_args {
            args.push(CString::new(arg).unwrap().into_raw());
        }

        // Get `R_HOME`, typically set by Positron / CI / kernel specification
        let r_home = match std::env::var("R_HOME") {
            Ok(home) => PathBuf::from(home),
            Err(_) => {
                // Get `R_HOME` from `PATH`, via R
                let Ok(result) = std::process::Command::new("R").arg("RHOME").output() else {
                    panic!("Can't find R or `R_HOME`");
                };
                let r_home = String::from_utf8(result.stdout).unwrap();
                let r_home = r_home.trim();
                unsafe { std::env::set_var("R_HOME", r_home) };
                PathBuf::from(r_home)
            },
        };

        let libraries = RLibraries::from_r_home_path(&r_home);
        libraries.initialize_pre_setup_r();

        crate::sys::interface::setup_r(args);

        libraries.initialize_post_setup_r();

        unsafe {
            // Register embedded routines
            r_register_routines();

            // Initialize harp (after routine registration)
            harp::initialize();

            // Optionally run a frontend specified R startup script (after harp init)
            if let Some(file) = &startup_file {
                harp::source(file)
                    .or_log_error(&format!("Failed to source startup file '{file}' due to"));
            }

            // Initialize support functions (after routine registration)
            match modules::initialize() {
                Err(err) => {
                    log::error!("Can't load R modules: {err:?}");
                },
                Ok(namespace) => {
                    r_main.positron_ns = Some(namespace);
                },
            }

            // Register all hooks once all modules have been imported
            let hook_result = RFunction::from(".ps.register_all_hooks").call();
            if let Err(err) = hook_result {
                log::error!("Error registering some hooks: {err:?}");
            }

            // Populate srcrefs for namespaces already loaded in the session.
            // Namespaces of future loaded packages will be populated on load.
            if do_resource_namespaces() {
                if let Err(err) = resource_loaded_namespaces() {
                    log::error!("Can't populate srcrefs for loaded packages: {err:?}");
                }
            }

            // Set up the global error handler (after support function initialization)
            errors::initialize();

            // Now that R has started (emitting any startup messages), and now that we have set
            // up all hooks and handlers, officially finish the R initialization process to
            // unblock the kernel-info request and also allow the LSP to start.
            log::info!(
                "R has started and ark handlers have been registered, completing initialization."
            );
            r_main.complete_initialization();
        }

        // Now that R has started and libr and ark have fully initialized, run site and user
        // level R profiles, in that order
        if !ignore_site_r_profile {
            startup::source_site_r_profile(&r_home);
        }
        if !ignore_user_r_profile {
            startup::source_user_r_profile();
        }
    }

    /// Start the REPL. Does not return!
    pub fn start() {
        // Set the main thread ID. We do it here so that `setup()` is allowed to
        // be called in another thread.
        unsafe { R_MAIN_THREAD_ID = Some(std::thread::current().id()) };
        crate::sys::interface::run_r();
    }

    /// Completes the kernel's initialization.
    /// Unlike `RMain::start()`, this has access to `R_MAIN`'s state, such as
    /// the kernel-info banner.
    /// SAFETY: Can only be called from the R thread, and only once.
    pub unsafe fn complete_initialization(&mut self) {
        let version = unsafe {
            let version = Rf_findVarInFrame(R_BaseNamespace, r_symbol!("R.version.string"));
            RObject::new(version).to::<String>().unwrap()
        };

        // Initial input and continuation prompts
        let input_prompt: String = harp::get_option("prompt").try_into().unwrap();
        let continuation_prompt: String = harp::get_option("continue").try_into().unwrap();

        let kernel_info = KernelInfo {
            version: version.clone(),
            banner: R_BANNER.clone(),
            input_prompt: Some(input_prompt),
            continuation_prompt: Some(continuation_prompt),
        };

        log::info!("Sending kernel info: {version}");
        self.kernel_init_tx.broadcast(kernel_info);

        // Thread-safe initialisation flag for R
        R_INIT.set(()).expect("`R_INIT` can only be set once");
    }

    pub fn new(
        kernel: Arc<Mutex<Kernel>>,
        tasks_interrupt_rx: Receiver<RTask>,
        tasks_idle_rx: Receiver<RTask>,
        comm_manager_tx: Sender<CommManagerEvent>,
        r_request_rx: Receiver<RRequest>,
        stdin_request_tx: Sender<StdInRequest>,
        stdin_reply_rx: Receiver<amalthea::Result<InputReply>>,
        iopub_tx: Sender<IOPubMessage>,
        kernel_init_tx: Bus<KernelInfo>,
        dap: Arc<Mutex<Dap>>,
        session_mode: SessionMode,
    ) -> Self {
        Self {
            r_request_rx,
            comm_manager_tx,
            stdin_request_tx,
            stdin_reply_rx,
            iopub_tx,
            kernel_init_tx,
            active_request: None,
            execution_count: 0,
            autoprint_output: String::new(),
            kernel,
            error_occurred: false,
            error_message: String::new(),
            error_traceback: Vec::new(),
            help_event_tx: None,
            help_port: None,
            lsp_events_tx: None,
            dap: RMainDap::new(dap),
            is_busy: false,
            tasks_interrupt_rx,
            tasks_idle_rx,
            pending_futures: HashMap::new(),
            session_mode,
            positron_ns: None,
            pending_lines: Vec::new(),
        }
    }

    /// Wait for complete R initialization
    ///
    /// Wait for R being ready to evaluate R code. Resolves as the same time as
    /// the `Bus<KernelInfo>` init channel does.
    ///
    /// Thread-safe.
    pub fn wait_initialized() {
        R_INIT.wait();
    }

    /// Has the `RMain` singleton completed initialization.
    ///
    /// This can return true when R might still not have finished starting up.
    /// See `wait_initialized()`.
    ///
    /// Thread-safe. But note you can only get access to the singleton on the R
    /// thread.
    pub fn is_initialized() -> bool {
        R_INIT.get().is_some()
    }

    /// Access a reference to the singleton instance of this struct
    ///
    /// SAFETY: Accesses must occur after `RMain::start()` initializes it, and must
    /// occur on the main R thread.
    pub fn get() -> &'static Self {
        RMain::get_mut()
    }

    /// Access a mutable reference to the singleton instance of this struct
    ///
    /// SAFETY: Accesses must occur after `RMain::start()` initializes it, and must
    /// occur on the main R thread.
    pub fn get_mut() -> &'static mut Self {
        if !RMain::on_main_thread() {
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("<unnamed>");
            let message =
                format!("Must access `R_MAIN` on the main R thread, not thread '{name}'.");
            #[cfg(debug_assertions)]
            panic!("{message}");
            #[cfg(not(debug_assertions))]
            log::error!("{message}");
        }

        unsafe {
            R_MAIN
                .as_mut()
                .expect("`R_MAIN` can't be used before it is initialized!")
        }
    }

    pub fn with<F, T>(f: F) -> T
    where
        F: FnOnce(&RMain) -> T,
    {
        let main = Self::get();
        f(main)
    }

    pub fn with_mut<F, T>(f: F) -> T
    where
        F: FnOnce(&mut RMain) -> T,
    {
        let main = Self::get_mut();
        f(main)
    }

    pub fn on_main_thread() -> bool {
        let thread = std::thread::current();
        thread.id() == unsafe { R_MAIN_THREAD_ID.unwrap() }
    }

    /// Provides read-only access to `iopub_tx`
    pub fn get_iopub_tx(&self) -> &Sender<IOPubMessage> {
        &self.iopub_tx
    }

    fn init_execute_request(&mut self, req: &ExecuteRequest) -> (ConsoleInput, u32) {
        // Reset the autoprint buffer
        self.autoprint_output = String::new();

        // Increment counter if we are storing this execution in history
        if req.store_history {
            self.execution_count = self.execution_count + 1;
        }

        // If the code is not to be executed silently, re-broadcast the
        // execution to all frontends
        if !req.silent {
            if let Err(err) = self.iopub_tx.send(IOPubMessage::ExecuteInput(ExecuteInput {
                code: req.code.clone(),
                execution_count: self.execution_count,
            })) {
                log::warn!(
                    "Could not broadcast execution input {} to all frontends: {}",
                    self.execution_count,
                    err
                );
            }
        }

        // Return the code to the R console to be evaluated and the corresponding exec count
        (ConsoleInput::Input(req.code.clone()), self.execution_count)
    }

    /// Invoked by R to read console input from the user.
    ///
    /// * `prompt` - The prompt shown to the user
    /// * `buf`    - Pointer to buffer to receive the user's input (type `CONSOLE_BUFFER_CHAR`)
    /// * `buflen` - Size of the buffer to receiver user's input
    /// * `hist`   - Whether to add the input to the history (1) or not (0)
    ///
    /// Returns a tuple. First value is to be passed on to `ReadConsole()` and
    /// indicates whether new input is available. Second value indicates whether
    /// we need to call `Rf_onintr()` to process an interrupt.
    fn read_console(
        &mut self,
        prompt: *const c_char,
        buf: *mut c_uchar,
        buflen: c_int,
        _hist: c_int,
    ) -> ConsoleResult {
        // We get called here everytime R needs more input. This handler
        // represents the driving event of a small state machine that manages
        // communication between R and the frontend:
        //
        // - If the vector of pending lines is empty, and if the prompt is for
        //   new R code, we close the active ExecuteRequest and send a response to
        //   the frontend.
        //
        // - If the vector of pending lines is not empty, R might be waiting for
        //   us to complete an incomplete expression, or we might just have
        //   completed an intermediate expression (e.g. from an ExecuteRequest
        //   like `foo\nbar` where `foo` is intermediate and `bar` is final).
        //   Send the next line to R.
        //
        // This state machine depends on being able to reliably distinguish
        // between readline prompts (from `readline()`, `scan()`, or `menu()`),
        // and actual R code prompts (either top-level or from a nested debug
        // REPL).  A readline prompt should never change our state (in
        // particular our vector of pending inputs). We think we are making this
        // distinction sufficiently robustly but ideally R would let us know the
        // prompt type so there is no ambiguity at all.
        //
        // R might throw an error at any time while we are working on our vector
        // of pending lines, either from a syntax error or from an evaluation
        // error. When this happens, we abort evaluation and clear the pending
        // lines.
        //
        // If the vector of pending lines is empty and we detect an incomplete
        // prompt, this is a panic. We check ahead of time for complete
        // expressions before breaking up an ExecuteRequest in multiple lines,
        // so this should not happen.

        if let Some(console_result) = self.handle_pending_line(buf, buflen) {
            return console_result;
        }

        let info = Self::prompt_info(prompt);
        log::trace!("R prompt: {}", info.input_prompt);

        // An incomplete prompt when we no longer have any inputs to send should
        // never happen because we check for incomplete inputs ahead of time and
        // respond to the frontend with an error.
        if info.incomplete && self.pending_lines.is_empty() {
            unreachable!("Incomplete input in `ReadConsole` handler");
        }

        // Upon entering read-console, finalize any debug call text that we were capturing.
        // At this point, the user can either advance the debugger, causing us to capture
        // a new expression, or execute arbitrary code, where we will reuse a finalized
        // debug call text to maintain the debug state.
        self.dap.finalize_call_text();

        // TODO: Can we remove this below code?
        // If the prompt begins with "Save workspace", respond with (n)
        //
        // NOTE: Should be able to overwrite the `Cleanup` frontend method.
        // This would also help with detecting normal exits versus crashes.
        if info.input_prompt.starts_with("Save workspace") {
            match Self::on_console_input(buf, buflen, String::from("n")) {
                Ok(()) => return ConsoleResult::NewInput,
                Err(err) => return ConsoleResult::Error(err),
            }
        }

        if info.input_request {
            if let Some(req) = &self.active_request {
                // Send request to frontend.  We'll wait for an `input_reply`
                // from the frontend in the event loop below. The active request
                // remains active.
                self.request_input(req.orig.clone(), info.input_prompt.to_string());
            } else {
                // Invalid input request, propagate error to R
                return self.handle_invalid_input_request(buf, buflen);
            }
        } else if let Some(req) = std::mem::take(&mut self.active_request) {
            // We got a prompt request marking the end of the previous
            // execution. We took and cleared the active request as we're about
            // to complete it and send a reply to unblock the active Shell
            // request.

            // FIXME: Race condition between the comm and shell socket threads.
            //
            // Send info for the next prompt to frontend. This handles
            // custom prompts set by users, e.g. `options(prompt = ,
            // continue = )`, as well as debugging prompts, e.g. after a
            // call to `browser()`.
            let event = UiFrontendEvent::PromptState(PromptStateParams {
                input_prompt: info.input_prompt.clone(),
                continuation_prompt: info.continuation_prompt.clone(),
            });
            {
                let kernel = self.kernel.lock().unwrap();
                kernel.send_ui_event(event);
            }

            // Let frontend know the last request is complete. This turns us
            // back to Idle.
            self.reply_execute_request(req, &info);
        }

        // In the future we'll also send browser information, see
        // https://github.com/posit-dev/positron/issues/3001. Currently this is
        // a push model where we send the console inputs at each round. In the
        // future, a pull model would be better, this way the LSP can manage a
        // cache of inputs and we don't need to retraverse the environments as
        // often. We'd still push a `DidChangeConsoleInputs` notification from
        // here, but only containing high-level information such as `search()`
        // contents and `ls(rho)`.
        if !info.browser && !info.incomplete && !info.input_request {
            self.refresh_lsp();
        }

        // Signal prompt
        EVENTS.console_prompt.emit(());

        if info.browser {
            match self.dap.stack_info() {
                Ok(stack) => {
                    self.dap.start_debug(stack);
                },
                Err(err) => log::error!("ReadConsole: Can't get stack info: {err}"),
            };
        } else {
            if self.dap.is_debugging() {
                // Terminate debugging session
                self.dap.stop_debug();
            }
        }

        loop {
            // If an interrupt was signaled and we are in a user
            // request prompt, e.g. `readline()`, we need to propagate
            // the interrupt to the R stack. This needs to happen before
            // `process_events()`, particularly on Windows, because it
            // calls `R_ProcessEvents()`, which checks and resets
            // `UserBreak`, but won't actually fire the interrupt b/c
            // we have them disabled, so it would end up swallowing the
            // user interrupt request.
            if info.input_request && interrupts_pending() {
                return ConsoleResult::Interrupt;
            }

            // Otherwise we are at top level and we can assume the
            // interrupt was 'handled' on the frontend side and so
            // reset the flag
            set_interrupts_pending(false);

            // FIXME: Race between interrupt and new code request. To fix
            // this, we could manage the Shell and Control sockets on the
            // common message event thread. The Control messages would need
            // to be handled in a blocking way to ensure subscribers are
            // notified before the next incoming message is processed.

            // First handle execute requests outside of `select!` to ensure they
            // have priority. `select!` chooses at random.
            if let Ok(req) = self.r_request_rx.try_recv() {
                if let Some(input) = self.handle_execute_request(req, &info, buf, buflen) {
                    return input;
                }
            }

            select! {
                // Wait for an execution request from the frontend.
                recv(self.r_request_rx) -> req => {
                    let Ok(req) = req else {
                        // The channel is disconnected and empty
                        return ConsoleResult::Disconnected;
                    };

                    if let Some(input) = self.handle_execute_request(req, &info, buf, buflen) {
                        return input;
                    }
                }

                // We've got a response for readline
                recv(self.stdin_reply_rx) -> reply => {
                    return self.handle_input_reply(reply.unwrap(), buf, buflen);
                }

                // A task woke us up, start next loop tick to yield to it
                recv(self.tasks_interrupt_rx) -> task => {
                    self.handle_task_interrupt(task.unwrap());
                }
                recv(self.tasks_idle_rx) -> task => {
                    self.handle_task(task.unwrap());
                }

                // Wait with a timeout. Necessary because we need to
                // pump the event loop while waiting for console input.
                //
                // Alternatively, we could try to figure out the file
                // descriptors that R has open and select() on those for
                // available data?
                default(Duration::from_millis(200)) => {
                    unsafe { Self::process_events() };
                }
            }
        }
    }

    // We prefer to panic if there is an error while trying to determine the
    // prompt type because any confusion here is prone to put the frontend in a
    // bad state (e.g. causing freezes)
    fn prompt_info(prompt_c: *const c_char) -> PromptInfo {
        let n_frame = harp::session::r_n_frame().unwrap();
        log::trace!("prompt_info(): n_frame = '{n_frame}'");

        let prompt_slice = unsafe { CStr::from_ptr(prompt_c) };
        let prompt = prompt_slice.to_string_lossy().into_owned();

        // Detect browser prompt by matching the prompt string
        // https://github.com/posit-dev/positron/issues/4742.
        // There are ways to break this detection, for instance setting
        // `options(prompt =, continue = ` to something that looks like
        // a browser prompt, or doing the same with `readline()`. We have
        // chosen to not support these edge cases.
        let browser = RE_DEBUG_PROMPT.is_match(&prompt);

        // If there are frames on the stack and we're not in a browser prompt,
        // this means some user code is requesting input, e.g. via `readline()`
        let user_request = !browser && n_frame > 0;

        // The request is incomplete if we see the continue prompt, except if
        // we're in a user request, e.g. `readline("+ ")`. To guard against
        // this, we check that we are at top-level (call stack is empty or we
        // have a debug prompt).
        let continuation_prompt: String = harp::get_option("continue").try_into().unwrap();
        let matches_continuation = prompt == continuation_prompt;
        let top_level = n_frame == 0 || browser;
        let incomplete = matches_continuation && top_level;

        return PromptInfo {
            input_prompt: prompt,
            continuation_prompt,
            browser,
            incomplete,
            input_request: user_request,
        };
    }

    fn handle_execute_request(
        &mut self,
        req: RRequest,
        info: &PromptInfo,
        buf: *mut c_uchar,
        buflen: c_int,
    ) -> Option<ConsoleResult> {
        if info.input_request {
            panic!("Unexpected `execute_request` while waiting for `input_reply`.");
        }

        let input = match req {
            RRequest::ExecuteCode(exec_req, orig, response_tx) => {
                // Extract input from request
                let (input, exec_count) = { self.init_execute_request(&exec_req) };

                // Save `ExecuteCode` request so we can respond to it at next prompt
                self.active_request = Some(ActiveReadConsoleRequest {
                    exec_count,
                    request: exec_req,
                    orig,
                    response_tx,
                });

                input
            },

            RRequest::Shutdown(_) => ConsoleInput::EOF,

            RRequest::DebugCommand(cmd) => {
                // Just ignore command in case we left the debugging state already
                if !self.dap.is_debugging() {
                    return None;
                }

                // Translate requests from the debugger frontend to actual inputs for
                // the debug interpreter
                ConsoleInput::Input(debug_request_command(cmd))
            },
        };

        // Clear error flag
        self.error_occurred = false;

        match input {
            ConsoleInput::Input(code) => {
                // Handle commands for the debug interpreter
                if self.dap.is_debugging() {
                    let continue_cmds = vec!["n", "f", "c", "cont"];
                    if continue_cmds.contains(&&code[..]) {
                        self.dap.send_dap(DapBackendEvent::Continued);
                    }
                }

                // If the input is invalid (e.g. incomplete), don't send it to R
                // at all, reply with an error right away
                if let Err(err) = Self::check_console_input(code.as_str()) {
                    return Some(ConsoleResult::Error(err));
                }

                // Split input by lines, retrieve first line, and store
                // remaining lines in a buffer. This helps with long inputs
                // because R has a fixed input buffer size of 4096 bytes at the
                // time of writing.
                let code = self.buffer_console_input(code.as_str());

                // Store input in R's buffer and return sentinel indicating some
                // new input is ready
                match Self::on_console_input(buf, buflen, code) {
                    Ok(()) => Some(ConsoleResult::NewInput),
                    Err(err) => Some(ConsoleResult::Error(err)),
                }
            },
            ConsoleInput::EOF => Some(ConsoleResult::Disconnected),
        }
    }

    /// Handle an `input_request` received outside of an `execute_request` context
    ///
    /// We believe it is always invalid to receive an `input_request` that isn't
    /// nested within an `execute_request`. However, this can happen at R
    /// startup when sourcing an `.Rprofile` that calls `readline()` or `menu()`.
    /// Both of these are explicitly forbidden by `?Startup` in R as
    /// "interaction with the user during startup", so when we detect this
    /// invalid `input_request` case we throw an R error and assume that it
    /// came from a `readline()` or `menu()` call during startup.
    ///
    /// We make a single exception for preexisting renv `activate.R` scripts,
    /// which used to call `readline()` from within `.Rprofile`. In those cases,
    /// we return `"n"` which allows older versions of renv to at least startup.
    /// https://github.com/posit-dev/positron/issues/2070
    /// https://github.com/rstudio/renv/blob/5d0d52c395e569f7f24df4288d949cef95efca4e/inst/resources/activate.R#L85-L87
    fn handle_invalid_input_request(&self, buf: *mut c_uchar, buflen: c_int) -> ConsoleResult {
        if Self::in_renv_autoloader() {
            log::info!("Detected `readline()` call in renv autoloader. Returning `'n'`.");
            match Self::on_console_input(buf, buflen, String::from("n")) {
                Ok(()) => return ConsoleResult::NewInput,
                Err(err) => return ConsoleResult::Error(err),
            }
        }

        log::info!("Detected invalid `input_request` outside an `execute_request`. Preparing to throw an R error.");

        let message = vec![
            "Can't request input from the user at this time.",
            "Are you calling `readline()` or `menu()` from an `.Rprofile` or `.Rprofile.site` file? If so, that is the issue and you should remove that code."
        ].join("\n");

        return ConsoleResult::Error(Error::InvalidInputRequest(message));
    }

    fn in_renv_autoloader() -> bool {
        harp::get_option("renv.autoloader.running")
            .try_into()
            .unwrap_or(false)
    }

    fn handle_input_reply(
        &self,
        reply: amalthea::Result<InputReply>,
        buf: *mut c_uchar,
        buflen: c_int,
    ) -> ConsoleResult {
        match reply {
            Ok(input) => {
                let input = convert_line_endings(&input.value, LineEnding::Posix);
                match Self::on_console_input(buf, buflen, input) {
                    Ok(()) => ConsoleResult::NewInput,
                    Err(err) => ConsoleResult::Error(err),
                }
            },
            Err(err) => ConsoleResult::Error(err),
        }
    }

    /// Handle a task at interrupt time.
    ///
    /// Wrapper around `handle_task()` that does some extra logging to record
    /// how long a task waited before being picked up by the R or ReadConsole
    /// event loop.
    ///
    /// Since tasks running during interrupt checks block the R thread while
    /// they are running, they should return very quickly. The log message helps
    /// monitor excessively long-running tasks.
    fn handle_task_interrupt(&mut self, mut task: RTask) {
        if let Some(start_info) = task.start_info_mut() {
            // Log excessive waiting before starting task
            if start_info.start_time.elapsed() > std::time::Duration::from_millis(50) {
                start_info.span.in_scope(|| {
                    tracing::info!(
                        "{} milliseconds wait before running task.",
                        start_info.start_time.elapsed().as_millis()
                    )
                });
            }

            // Reset timer, next time we'll log how long the task took
            start_info.start_time = std::time::Instant::now();
        }

        let finished_task_info = self.handle_task(task);

        // We only log long task durations in the interrupt case since we expect
        // idle tasks to take longer. Use the tracing profiler to monitor the
        // duration of idle tasks.
        if let Some(info) = finished_task_info {
            if info.elapsed() > std::time::Duration::from_millis(50) {
                info.span.in_scope(|| {
                    log::info!("task took {} milliseconds.", info.elapsed().as_millis());
                })
            }
        }
    }

    /// Returns start information when the task has been completed
    fn handle_task(&mut self, task: RTask) -> Option<RTaskStartInfo> {
        // Background tasks can't take any user input, so we set R_Interactive
        // to 0 to prevent `readline()` from blocking the task.
        let _interactive = harp::raii::RLocalInteractive::new(false);

        match task {
            RTask::Sync(task) => {
                // Immediately let caller know we have started so it can set up the
                // timeout
                if let Some(ref status_tx) = task.status_tx {
                    status_tx.send(RTaskStatus::Started).unwrap();
                }

                let result = task.start_info.span.in_scope(|| r_sandbox(task.fun));

                // Unblock caller via the notification channel
                if let Some(ref status_tx) = task.status_tx {
                    status_tx.send(RTaskStatus::Finished(result)).unwrap()
                }

                Some(task.start_info)
            },

            RTask::Async(task) => {
                let id = Uuid::new_v4();
                let waker = Arc::new(r_task::RTaskWaker {
                    id,
                    tasks_tx: task.tasks_tx.clone(),
                    start_info: task.start_info,
                });
                self.poll_task(Some(task.fut), waker)
            },

            RTask::Parked(waker) => self.poll_task(None, waker),
        }
    }

    fn poll_task(
        &mut self,
        fut: Option<BoxFuture<'static, ()>>,
        waker: Arc<r_task::RTaskWaker>,
    ) -> Option<r_task::RTaskStartInfo> {
        let tick = std::time::Instant::now();

        let (mut fut, mut start_info) = match fut {
            Some(fut) => (fut, waker.start_info.clone()),
            None => self.pending_futures.remove(&waker.id).unwrap(),
        };

        let awaker = waker.clone().into();
        let mut ctxt = &mut std::task::Context::from_waker(&awaker);

        match waker
            .start_info
            .span
            .in_scope(|| r_sandbox(|| fut.as_mut().poll(&mut ctxt)).unwrap())
        {
            Poll::Ready(()) => {
                start_info.bump_elapsed(tick.elapsed());
                Some(start_info)
            },
            Poll::Pending => {
                start_info.bump_elapsed(tick.elapsed());
                self.pending_futures.insert(waker.id, (fut, start_info));
                None
            },
        }
    }

    fn handle_pending_line(&mut self, buf: *mut c_uchar, buflen: c_int) -> Option<ConsoleResult> {
        if self.error_occurred {
            // If an error has occurred, we've already sent a complete expression that resulted in
            // an error. Flush the remaining lines and return to `read_console()`, who will handle
            // that error.
            self.pending_lines.clear();
            return None;
        }

        let Some(input) = self.pending_lines.pop() else {
            // No pending lines
            return None;
        };

        match Self::on_console_input(buf, buflen, input) {
            Ok(()) => Some(ConsoleResult::NewInput),
            Err(err) => Some(ConsoleResult::Error(err)),
        }
    }

    fn check_console_input(input: &str) -> amalthea::Result<()> {
        let status = unwrap!(harp::parse_status(&harp::ParseInput::Text(input)), Err(err) => {
            // Failed to even attempt to parse the input, something is seriously wrong
            return Err(Error::InvalidConsoleInput(format!(
                "Failed to parse input: {err:?}"
            )));
        });

        // - Incomplete inputs put R into a state where it expects more input that will never come, so we
        //   immediately reject them. Positron should never send us these, but Jupyter Notebooks may.
        // - Complete statements are obviously fine.
        // - Syntax errors will cause R to throw an error, which is expected.
        match status {
            harp::ParseResult::Incomplete => Err(Error::InvalidConsoleInput(format!(
                "Can't execute incomplete input:\n{input}"
            ))),
            harp::ParseResult::Complete(_) => Ok(()),
            harp::ParseResult::SyntaxError { .. } => Ok(()),
        }
    }

    fn buffer_console_input(&mut self, input: &str) -> String {
        // Split into lines and reverse them to be able to `pop()` from the front
        let mut lines: Vec<String> = lines(input).rev().map(String::from).collect();

        // SAFETY: There is always at least one line because:
        // - `lines("")` returns 1 element containing `""`
        // - `lines("\n")` returns 2 elements containing `""`
        let first = lines.pop().unwrap();

        // No-op if `lines` is empty
        assert!(self.pending_lines.is_empty());
        self.pending_lines.append(&mut lines);

        first
    }

    /// Copy console input into R's internal input buffer
    ///
    /// Supposedly `buflen` is "the maximum length, in bytes, including the
    /// terminator". In practice it seems like R adds 1 extra byte on top of
    /// this when allocating the buffer, but we don't abuse that.
    /// https://github.com/wch/r-source/blob/20c9590fd05c54dba6c9a1047fb0ba7822ba8ba2/src/include/Defn.h#L1863-L1865
    ///
    /// Due to `buffer_console_input()`, we should only ever write 1 line of
    /// console input to R's internal buffer at a time. R calls
    /// `read_console()` back if it needs more input, allowing us to provide
    /// the next line.
    ///
    /// In the case of receiving too much input within a SINGLE line, we
    /// propagate up an informative `amalthea::Error::InvalidConsoleInput`
    /// error, which is turned into an R error and thrown in a POD context.
    /// This is a fairly pathological case that we never expect to occur.
    fn on_console_input(
        buf: *mut c_uchar,
        buflen: c_int,
        mut input: String,
    ) -> amalthea::Result<()> {
        let buflen = buflen as usize;

        if buflen < 2 {
            // Pathological case. A user wouldn't be able to do anything useful anyways.
            panic!("Console input `buflen` must be >=2.");
        }

        // Leave room for final `\n` and `\0` terminator
        let buflen = buflen - 2;

        if input.len() > buflen {
            log::error!("Console input too large for buffer, throwing R error.");
            return Err(Self::buffer_overflow_error());
        }

        // Push `\n`
        input.push('\n');

        // Push `\0` (automatically, as it converts to a C string)
        let input = CString::new(input).unwrap();

        unsafe {
            libc::strcpy(buf as *mut c_char, input.as_ptr());
        }

        Ok(())
    }

    // Hitting this means a SINGLE line from the user was longer than the buffer size (>4000 characters)
    fn buffer_overflow_error() -> amalthea::Error {
        Error::InvalidConsoleInput(String::from(
            "Can't pass console input on to R, a single line exceeds R's internal console buffer size."
        ))
    }

    // Reply to the previously active request. The current prompt type and
    // whether an error has occurred defines the response kind.
    fn reply_execute_request(&mut self, req: ActiveReadConsoleRequest, prompt_info: &PromptInfo) {
        let prompt = &prompt_info.input_prompt;

        let (response, result) = if prompt_info.incomplete {
            log::trace!("Got prompt {} signaling incomplete request", prompt);
            (new_incomplete_response(&req.request, req.exec_count), None)
        } else if prompt_info.input_request {
            unreachable!();
        } else {
            log::trace!("Got R prompt '{}', completing execution", prompt);

            self.make_execute_response_error(req.exec_count)
                .unwrap_or_else(|| self.make_execute_response_result(req.exec_count))
        };

        if let Some(result) = result {
            self.iopub_tx.send(result).unwrap();
        }

        log::trace!("Sending `execute_response`: {response:?}");
        req.response_tx.send(response).unwrap();
    }

    fn make_execute_response_error(
        &mut self,
        exec_count: u32,
    ) -> Option<(ExecuteResponse, Option<IOPubMessage>)> {
        // Save and reset error occurred flag
        let error_occurred = self.error_occurred;
        self.error_occurred = false;

        // Error handlers are not called on stack overflow so the error flag
        // isn't set. Instead we detect stack overflows by peeking at the error
        // buffer. The message is explicitly not translated to save stack space
        // so the matching should be reliable.
        let err_buf = r_peek_error_buffer();
        let stack_overflow_occurred = RE_STACK_OVERFLOW.is_match(&err_buf);

        // Reset error buffer so we don't display this message again
        if stack_overflow_occurred {
            let _ = RFunction::new("base", "stop").call();
        }

        // Send the reply to the frontend
        if !error_occurred && !stack_overflow_occurred {
            return None;
        }

        // We don't fill out `ename` with anything meaningful because typically
        // R errors don't have names. We could consider using the condition class
        // here, which r-lib/tidyverse packages have been using more heavily.
        let mut exception = if error_occurred {
            Exception {
                ename: String::from(""),
                evalue: self.error_message.clone(),
                traceback: self.error_traceback.clone(),
            }
        } else {
            // Call `base::traceback()` since we don't have a handled error
            // object carrying a backtrace. This won't be formatted as a
            // tree which is just as well since the recursive calls would
            // push a tree too far to the right.
            let traceback = r_traceback();
            Exception {
                ename: String::from(""),
                evalue: err_buf.clone(),
                traceback,
            }
        };

        // Jupyter clients typically discard the `evalue` when a `traceback` is
        // present.  Jupyter-Console even disregards `evalue` in all cases. So
        // include it here if we are in Notebook mode. But should Positron
        // implement similar behaviour as the other frontends eventually? The
        // first component of `traceback` could be compared to `evalue` and
        // discarded from the traceback if the same.
        if let SessionMode::Notebook = self.session_mode {
            exception.traceback.insert(0, exception.evalue.clone())
        }

        let response = new_execute_response_error(exception.clone(), exec_count);
        let result = IOPubMessage::ExecuteError(ExecuteError { exception });

        Some((response, Some(result)))
    }

    fn make_execute_response_result(
        &mut self,
        exec_count: u32,
    ) -> (ExecuteResponse, Option<IOPubMessage>) {
        // TODO: Implement rich printing of certain outputs.
        // Will we need something similar to the RStudio model,
        // where we implement custom print() methods? Or can
        // we make the stub below behave sensibly even when
        // streaming R output?
        let mut data = serde_json::Map::new();

        // The output generated by autoprint is emitted as an
        // `execute_result` message.
        let mut autoprint = std::mem::take(&mut self.autoprint_output);

        if autoprint.ends_with('\n') {
            // Remove the trailing newlines that R adds to outputs but that
            // Jupyter frontends are not expecting. Is it worth taking a
            // mutable self ref across calling methods to avoid the clone?
            autoprint.pop();
        }
        if autoprint.len() != 0 {
            data.insert("text/plain".to_string(), json!(autoprint));
        }

        // Include HTML representation of data.frame
        unsafe {
            let value = Rf_findVarInFrame(R_GlobalEnv, r_symbol!(".Last.value"));
            if r_is_data_frame(value) {
                match to_html(value) {
                    Ok(html) => data.insert("text/html".to_string(), json!(html)),
                    Err(err) => {
                        log::error!("{:?}", err);
                        None
                    },
                };
            }
        }

        let response = new_execute_response(exec_count);

        let result = (data.len() > 0).then(|| {
            IOPubMessage::ExecuteResult(ExecuteResult {
                execution_count: exec_count,
                data: serde_json::Value::Object(data),
                metadata: json!({}),
            })
        });

        (response, result)
    }

    /// Sends a `Wait` message to IOPub, which responds when the IOPub thread
    /// actually processes the message, implying that all other IOPub messages
    /// in front of this one have been forwarded on to the frontend.
    /// TODO: Remove this when we can, see `request_input()` for rationale.
    fn wait_for_empty_iopub(&self) {
        let (wait_tx, wait_rx) = bounded::<()>(1);

        let message = IOPubMessage::Wait(Wait { wait_tx });

        if let Err(error) = self.iopub_tx.send(message) {
            log::error!("Failed to send wait request to iopub: {error:?}");
            return;
        }

        if let Err(error) = wait_rx.recv() {
            log::error!("Failed to receive wait response from iopub: {error:?}");
        }
    }

    /// Request input from frontend in case code like `readline()` is
    /// waiting for input
    fn request_input(&self, orig: Option<Originator>, prompt: String) {
        // TODO: We really should not have to wait on IOPub to be cleared, but
        // if an IOPub `'stream'` message arrives on the frontend while an input
        // request is being handled, it currently breaks the Console. We should
        // remove `wait_on_empty_iopub()` once this is fixed:
        // https://github.com/posit-dev/positron/issues/1700
        // https://github.com/posit-dev/amalthea/pull/131
        self.wait_for_empty_iopub();

        // TODO: Remove this too. Unfortunately even if we wait for the IOPub
        // queue to clear, that doesn't guarantee the frontend has processed
        // all of the messages in the queue, only that they have been send over.
        // So the input request (sent over the stdin socket) can STILL arrive
        // before all of the IOPub messages have been processed by the frontend.
        std::thread::sleep(std::time::Duration::from_millis(100));

        unwrap!(
            self.stdin_request_tx
            .send(StdInRequest::Input(ShellInputRequest {
                originator: orig,
                request: InputRequest {
                    prompt,
                    password: false,
                },
            })),
            Err(err) => panic!("Could not send input request: {}", err)
        )
    }

    /// Invoked by R to write output to the console.
    fn write_console(buf: *const c_char, _buflen: i32, otype: i32) {
        let content = match console_to_utf8(buf) {
            Ok(content) => content,
            Err(err) => panic!("Failed to read from R buffer: {err:?}"),
        };

        if !RMain::is_initialized() {
            // During init, consider all output to be part of the startup banner
            unsafe { R_BANNER.push_str(&content) };
            return;
        }

        let r_main = RMain::get_mut();

        // To capture the current `debug: <call>` output, for use in the debugger's
        // match based fallback
        r_main.dap.handle_stdout(&content);

        let stream = if otype == 0 {
            Stream::Stdout
        } else {
            Stream::Stderr
        };

        // If active execution request is silent don't broadcast
        // any output
        if let Some(ref req) = r_main.active_request {
            if req.request.silent {
                return;
            }
        }

        if stream == Stream::Stdout && is_auto_printing() {
            // If we are at top-level, we're handling visible output auto-printed by
            // the R REPL. We accumulate this output (it typically comes in multiple
            // parts) so we can emit it later on as part of the execution reply
            // message sent to Shell, as opposed to an Stdout message sent on IOPub.
            //
            // However, if autoprint is dealing with an intermediate expression
            // (i.e. `a` and `b` in `a\nb\nc`), we should emit it on IOPub. We
            // only accumulate autoprint output for the very last expression. The
            // way to distinguish between these cases is whether there are still
            // lines of input pending. In that case, that means we are on an
            // intermediate expression.
            //
            // Note that we implement this behaviour (streaming autoprint results of
            // intermediate expressions) specifically for Positron, and specifically
            // for versions that send multiple expressions selected by the user in
            // one request. Other Jupyter frontends do not want to see output for
            // these intermediate expressions. And future versions of Positron will
            // never send multiple expressions in one request
            // (https://github.com/posit-dev/positron/issues/1326).
            //
            // Note that warnings emitted lazily on stdout will appear to be part of
            // autoprint. We currently emit them on stderr, which allows us to
            // differentiate, but that could change in the future:
            // https://github.com/posit-dev/positron/issues/1881

            // Handle last expression
            if r_main.pending_lines.is_empty() {
                r_main.autoprint_output.push_str(&content);
                return;
            }

            // In notebooks, we don't emit results of intermediate expressions
            if r_main.session_mode == SessionMode::Notebook {
                return;
            }

            // In Positron, fall through if we have pending input. This allows
            // autoprint output for intermediate expressions to be emitted on
            // IOPub.
        }

        // Stream output via the IOPub channel.
        let message = IOPubMessage::Stream(StreamOutput {
            name: stream,
            text: content,
        });
        r_main.iopub_tx.send(message).unwrap();
    }

    /// Invoked by R to change busy state
    fn busy(&mut self, which: i32) {
        // Ensure signal handlers are initialized.
        //
        // Does nothing on Windows.
        //
        // We perform this awkward dance because R tries to set and reset
        // the interrupt signal handler here, using 'signal()':
        //
        // https://github.com/wch/r-source/blob/e7a21904029917a63b4717b53a173b01eeabcc7b/src/unix/sys-std.c#L171-L178
        //
        // However, it seems like this can cause the old interrupt handler to be
        // 'moved' to a separate thread, such that interrupts end up being handled
        // on a thread different from the R execution thread. At least, on macOS.
        initialize_signal_handlers();

        // Create an event representing the new busy state
        self.is_busy = which != 0;
        let event = UiFrontendEvent::Busy(BusyParams { busy: self.is_busy });

        // Wait for a lock on the kernel and have it deliver the event to
        // the frontend
        let kernel = self.kernel.lock().unwrap();
        kernel.send_ui_event(event);
    }

    /// Invoked by R to show a message to the user.
    fn show_message(&self, buf: *const c_char) {
        let message = unsafe { CStr::from_ptr(buf) };

        // Create an event representing the message
        let event = UiFrontendEvent::ShowMessage(ShowMessageParams {
            message: message.to_str().unwrap().to_string(),
        });

        // Wait for a lock on the kernel and have the kernel deliver the
        // event to the frontend
        let kernel = self.kernel.lock().unwrap();
        kernel.send_ui_event(event);
    }

    /// Invoked by the R event loop
    fn polled_events(&mut self) {
        // Skip running tasks if we don't have 128KB of stack space available.
        // This is 1/8th of the typical Windows stack space (1MB, whereas macOS
        // and Linux have 8MB).
        if let Err(_) = r_check_stack(Some(128 * 1024)) {
            return;
        }

        // Coalesce up to three concurrent tasks in case the R event loop is
        // slowed down
        for _ in 0..3 {
            if let Ok(task) = self.tasks_interrupt_rx.try_recv() {
                self.handle_task_interrupt(task);
            } else {
                break;
            }
        }
    }

    unsafe fn process_events() {
        // Process regular R events. We're normally running with polled
        // events disabled so that won't run here. We also run with
        // interrupts disabled, so on Windows those won't get run here
        // either (i.e. if `UserBreak` is set), but it will reset `UserBreak`
        // so we need to ensure we handle interrupts right before calling
        // this.
        R_ProcessEvents();

        crate::sys::interface::run_activity_handlers();

        // Run pending finalizers. We need to do this eagerly as otherwise finalizers
        // might end up being executed on the LSP thread.
        // https://github.com/rstudio/positron/issues/431
        R_RunPendingFinalizers();

        // Check for Positron render requests
        graphics_device::on_process_events();
    }

    pub fn get_comm_manager_tx(&self) -> &Sender<CommManagerEvent> {
        // Read only access to `comm_manager_tx`
        &self.comm_manager_tx
    }

    pub fn get_kernel(&self) -> &Arc<Mutex<Kernel>> {
        &self.kernel
    }

    pub(crate) fn set_help_fields(&mut self, help_event_tx: Sender<HelpEvent>, help_port: u16) {
        self.help_event_tx = Some(help_event_tx);
        self.help_port = Some(help_port);
    }

    pub(crate) fn send_help_event(&self, event: HelpEvent) -> anyhow::Result<()> {
        let Some(ref tx) = self.help_event_tx else {
            return Err(anyhow!("No help channel available to handle help event. Is the help comm open? Event {event:?}."));
        };

        if let Err(err) = tx.send(event) {
            return Err(anyhow!("Failed to send help message: {err:?}"));
        }

        Ok(())
    }

    pub(crate) fn is_help_url(&self, url: &str) -> bool {
        let Some(port) = self.help_port else {
            log::error!("No help port is available to check if '{url}' is a help url. Is the help comm open?");
            // Fail to recognize this as a help url, allow any fallbacks methods to run instead.
            return false;
        };

        RHelp::is_help_url(url, port)
    }

    fn send_lsp_notification(&self, event: KernelNotification) {
        if let Some(ref tx) = self.lsp_events_tx {
            tx.send(Event::Kernel(event)).unwrap();
        }
    }

    pub(crate) fn set_lsp_channel(&mut self, lsp_events_tx: TokioUnboundedSender<Event>) {
        self.lsp_events_tx = Some(lsp_events_tx.clone());

        // Refresh LSP state now since we probably have missed some updates
        // while the channel was offline. This is currently not an ideal timing
        // as the channel is set up from a preemptive `r_task()` after the LSP
        // is set up. We'll want to do this in an idle task.
        self.refresh_lsp();
    }

    pub fn refresh_lsp(&self) {
        match console_inputs() {
            Ok(inputs) => {
                self.send_lsp_notification(KernelNotification::DidChangeConsoleInputs(inputs));
            },
            Err(err) => log::error!("Can't retrieve console inputs: {err:?}"),
        }
    }

    pub fn call_frontend_method(&self, request: UiFrontendRequest) -> anyhow::Result<RObject> {
        log::trace!("Calling frontend method '{request:?}'");
        let (response_tx, response_rx) = bounded(1);

        let originator = if let Some(req) = &self.active_request {
            req.orig.clone()
        } else {
            anyhow::bail!("Error: No active request");
        };

        let comm_request = UiCommFrontendRequest {
            originator,
            response_tx,
            request: request.clone(),
        };

        // Send request via Kernel
        {
            let kernel = self.kernel.lock().unwrap();
            kernel.send_ui_request(comm_request);
        }

        // Block for response
        let response = response_rx.recv().unwrap();

        log::trace!("Got response from frontend method: {response:?}");

        match response {
            StdInRpcReply::Response(response) => match response {
                JsonRpcReply::Result(response) => {
                    // Deserialize to Rust first to verify the OpenRPC contract.
                    // Errors are propagated to R.
                    if let Err(err) =
                        ui_frontend_reply_from_value(response.result.clone(), &request)
                    {
                        anyhow::bail!("Can't deserialize RPC response for {request:?}:\n{err:?}");
                    }

                    // Now deserialize to an R object
                    Ok(RObject::try_from(response.result)?)
                },
                JsonRpcReply::Error(response) => anyhow::bail!(
                    "While calling frontend method:\n\
                     {}",
                    response.error.message
                ),
            },
            // If an interrupt was signalled, return `NULL`. This should not be
            // visible to the caller since `r_unwrap()` (called e.g. by
            // `harp::register`) will trigger an interrupt jump right away.
            StdInRpcReply::Interrupt => Ok(RObject::null()),
        }
    }

    pub fn send_frontend_event(&self, event: UiFrontendEvent) {
        log::trace!("Sending frontend event '{event:?}'");
        // Send request via Kernel
        let kernel = self.kernel.lock().unwrap();
        kernel.send_ui_event(event);
    }
}

/// Report an incomplete request to the frontend
fn new_incomplete_response(req: &ExecuteRequest, exec_count: u32) -> ExecuteResponse {
    ExecuteResponse::ReplyException(ExecuteReplyException {
        status: Status::Error,
        execution_count: exec_count,
        exception: Exception {
            ename: "IncompleteInput".to_string(),
            evalue: format!("Code fragment is not complete: {}", req.code),
            traceback: vec![],
        },
    })
}

static RE_STACK_OVERFLOW: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"C stack usage [ 0-9]+ is too close to the limit\n").unwrap());

fn new_execute_response(exec_count: u32) -> ExecuteResponse {
    ExecuteResponse::Reply(ExecuteReply {
        status: Status::Ok,
        execution_count: exec_count,
        user_expressions: json!({}),
    })
}
fn new_execute_response_error(exception: Exception, exec_count: u32) -> ExecuteResponse {
    ExecuteResponse::ReplyException(ExecuteReplyException {
        status: Status::Error,
        execution_count: exec_count,
        exception,
    })
}

/// Converts a data frame to HTML
fn to_html(frame: SEXP) -> Result<String> {
    unsafe {
        let result = RFunction::from(".ps.format.toHtml")
            .add(frame)
            .call()?
            .to::<String>()?;
        Ok(result)
    }
}

// Inputs generated by `ReadConsole` for the LSP
pub(crate) fn console_inputs() -> anyhow::Result<ConsoleInputs> {
    // TODO: Should send the debug environment if debugging:
    // https://github.com/posit-dev/positron/issues/3001
    let env = Environment::new(R_ENVS.global.into());
    let scopes = env.ancestors().map(|e| e.names()).collect();

    // Get the set of installed packages
    let installed_packages: Vec<String> = RFunction::new("base", ".packages")
        .param("all.available", true)
        .call()?
        .try_into()?;

    Ok(ConsoleInputs {
        console_scopes: scopes,
        installed_packages,
    })
}

// --- Frontend methods ---
// These functions are hooked up as R frontend methods. They call into our
// global `RMain` singleton.

#[no_mangle]
pub extern "C" fn r_read_console(
    prompt: *const c_char,
    buf: *mut c_uchar,
    buflen: c_int,
    hist: c_int,
) -> i32 {
    let main = RMain::get_mut();
    let result = r_sandbox(|| main.read_console(prompt, buf, buflen, hist));

    let result = unwrap!(result, Err(err) => {
        panic!("Unexpected longjump while reading console: {err:?}");
    });

    // NOTE: Keep this function a "Plain Old Frame" without any
    // destructors. We're longjumping from here in case of interrupt.

    match result {
        ConsoleResult::NewInput => return 1,
        ConsoleResult::Disconnected => return 0,
        ConsoleResult::Interrupt => {
            log::trace!("Interrupting `ReadConsole()`");
            unsafe {
                Rf_onintr();
            }

            // This normally does not return
            log::error!("`Rf_onintr()` did not longjump");
            return 0;
        },
        ConsoleResult::Error(err) => {
            // Save error message to a global buffer to avoid leaking memory
            // when `Rf_error()` jumps. Some gymnastics are required to deal
            // with the possibility of `CString` conversion failure since the
            // error message comes from the frontend and might be corrupted.
            static mut ERROR_BUF: Option<CString> = None;

            unsafe {
                ERROR_BUF = Some(new_cstring(format!("\n{err}")));
            }

            unsafe { Rf_error(ERROR_BUF.as_ref().unwrap().as_ptr()) };
        },
    };
}

fn new_cstring(x: String) -> CString {
    CString::new(x).unwrap_or(CString::new("Can't create CString").unwrap())
}

#[no_mangle]
pub extern "C" fn r_write_console(buf: *const c_char, buflen: i32, otype: i32) {
    RMain::write_console(buf, buflen, otype);
}

#[no_mangle]
pub extern "C" fn r_show_message(buf: *const c_char) {
    let main = RMain::get();
    main.show_message(buf);
}

#[no_mangle]
pub extern "C" fn r_busy(which: i32) {
    let main = RMain::get_mut();
    main.busy(which);
}

#[no_mangle]
pub extern "C" fn r_suicide(buf: *const c_char) {
    let msg = unsafe { CStr::from_ptr(buf) };
    panic!("Suicide: {}", msg.to_str().unwrap());
}

#[no_mangle]
pub unsafe extern "C" fn r_polled_events() {
    let main = RMain::get_mut();
    main.polled_events();
}

// This hook is called like a user onLoad hook but for every package to be
// loaded in the session
#[harp::register]
unsafe extern "C" fn ps_onload_hook(pkg: SEXP, _path: SEXP) -> anyhow::Result<SEXP> {
    // NOTE: `_path` might be NULL for a compat reason, see comments on the R side

    let pkg: String = RObject::view(pkg).try_into()?;

    // Need to reset parent as this might run in the context of another thread's R task
    let _span = tracing::trace_span!(parent: None, "onload_hook", pkg = pkg).entered();

    // Populate fake source refs if needed
    if do_resource_namespaces() {
        r_task::spawn_idle(|| async move {
            if let Err(err) = ns_populate_srcref(pkg.clone()).await {
                log::error!("Can't populate srcref for `{pkg}`: {err:?}");
            }
        });
    }

    Ok(RObject::null().sexp)
}

fn do_resource_namespaces() -> bool {
    // Don't slow down integration tests with srcref generation
    if stdext::IS_TESTING {
        return false;
    }

    let opt: Option<bool> = r_null_or_try_into(harp::get_option("ark.resource_namespaces"))
        .ok()
        .flatten();
    opt.unwrap_or(true)
}

/// Are we auto-printing?
///
/// We consider that we are auto-printing when the call stack is empty or when
/// the first frame on the stack is a call to `print()` with the function
/// inlined in CAR (it just so happens that this is how R constructs this call
/// for objects requiring dispatch - this heuristic can lead to unexpected
/// behaviour in edge cases). See:
/// https://github.com/wch/r-source/blob/bb7081cde24feeb59de9542018e31c14641e019e/src/main/print.c#L359-L38
///
/// We don't currently detect auto-printing in browser sessions as this is a bit
/// tricky.
///
/// Ideally R would pass this information as part of an extended
/// `WriteConsoleExt()` method so that we don't have to rely on these fragile
/// and incomplete inferences.
fn is_auto_printing() -> bool {
    let n_frame = harp::session::r_n_frame().unwrap();

    // The call-stack is empty so this must be R auto-printing an unclassed object
    if n_frame == 0 {
        return true;
    }

    // Disabled for now because it might cause unexpected output behaviour in Positron
    // // Are we auto-printing in a browser session? Incomplete heuristic.
    // let last_frame = harp::session::r_sys_frame(n_frame).unwrap();
    // let browser = harp::session::r_env_is_browsed(last_frame.sexp).unwrap();

    // Detect the `print()` call generated by auto-print with classed objects.
    // In tat case the first frame of the stack is a call to `print()` with the
    // function inlined in CAR. This inlining disambiguates with the user typing
    // a `print()` call at top-level. (Similar logic for the S4 generic `show()`.)
    let call = harp::session::r_sys_call(1).unwrap();

    // For safety
    if r_typeof(call.sexp) != libr::LANGSXP {
        return false;
    }

    unsafe {
        let car = libr::CAR(call.sexp);

        let Ok(print_fun) = harp::try_eval(r_symbol!("print"), R_ENVS.base) else {
            return false;
        };
        if car == print_fun.sexp {
            return true;
        }

        let Ok(methods_ns) = r_ns_env("methods") else {
            return false;
        };
        let Ok(show_fun) = harp::try_eval(r_symbol!("show"), methods_ns.into()) else {
            return false;
        };
        car == show_fun.sexp
    }
}
