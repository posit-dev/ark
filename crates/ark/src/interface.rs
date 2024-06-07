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
use std::sync::Once;
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
use anyhow::*;
use bus::Bus;
use crossbeam::channel::bounded;
use crossbeam::channel::unbounded;
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use crossbeam::select;
use harp::environment::Environment;
use harp::environment::R_ENVS;
use harp::exec::geterrmessage;
use harp::exec::r_check_stack;
use harp::exec::r_sandbox;
use harp::exec::r_source;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::library::RLibraries;
use harp::line_ending::convert_line_endings;
use harp::line_ending::LineEnding;
use harp::object::RObject;
use harp::r_symbol;
use harp::routines::r_register_routines;
use harp::session::r_traceback;
use harp::utils::r_get_option;
use harp::utils::r_is_data_frame;
use harp::utils::r_pairlist_any;
use harp::utils::r_poke_option_show_error_messages;
use harp::R_MAIN_THREAD_ID;
use libr::R_BaseNamespace;
use libr::R_GlobalEnv;
use libr::R_ProcessEvents;
use libr::R_RunPendingFinalizers;
use libr::Rf_error;
use libr::Rf_findVarInFrame;
use libr::Rf_onintr;
use libr::SEXP;
use log::*;
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
use crate::help::message::HelpReply;
use crate::help::message::HelpRequest;
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
use crate::sys::console::console_to_utf8;

// --- Globals ---
// These values must be global in order for them to be accessible from R
// callbacks, which do not have a facility for passing or returning context.

/// Ensures that the kernel is only ever initialized once
static INIT: Once = Once::new();
static INIT_KERNEL: Once = Once::new();

// The global state used by R callbacks.
//
// Doesn't need a mutex because it's only accessed by the R thread. Should
// not be used elsewhere than from an R frontend callback or an R function
// invoked by the REPL (this is enforced by `RMain::get()` and
// `RMain::get_mut()`).
static mut R_MAIN: Option<RMain> = None;

/// Starts the main R thread. Doesn't return.
pub fn start_r(
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
) {
    // Initialize global state (ensure we only do this once!)
    INIT.call_once(|| unsafe {
        R_MAIN_THREAD_ID = Some(std::thread::current().id());

        // Channels to send/receive tasks from auxiliary threads via `RTask`s
        let (tasks_interrupt_tx, tasks_interrupt_rx) = unbounded::<RTask>();
        let (tasks_idle_tx, tasks_idle_rx) = unbounded::<RTask>();

        r_task::initialize(tasks_interrupt_tx.clone(), tasks_idle_tx.clone());

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
        ));
    });

    // Build the argument list from the command line arguments. The default
    // list is `--interactive` unless altered with the `--` passthrough
    // argument.
    let mut args = cargs!["ark"];
    for arg in r_args {
        args.push(CString::new(arg).unwrap().into_raw());
    }

    // Get `R_HOME`, set by Positron / CI / kernel specification
    let r_home = match std::env::var("R_HOME") {
        Ok(home) => PathBuf::from(home),
        Err(err) => panic!("Can't find `R_HOME`: {err:?}"),
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

        // Optionally run a user specified R startup script (after harp init)
        if let Some(file) = &startup_file {
            r_source(file).or_log_error(&format!("Failed to source startup file '{file}' due to"));
        }

        // Initialize support functions (after routine registration)
        if let Err(err) = modules::initialize(false) {
            log::error!("Can't load R modules: {err:?}");
        }

        // Register all hooks once all modules have been imported
        let hook_result = RFunction::from(".ps.register_all_hooks").call();
        if let Err(err) = hook_result {
            log::error!("Error registering some hooks: {err:?}");
        }

        // Set up the global error handler (after support function initialization)
        errors::initialize();
    }

    // Does not return!
    crate::sys::interface::run_r();
}

pub struct RMain {
    initializing: bool,
    kernel_init_tx: Bus<KernelInfo>,

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

    stdout: String,
    stderr: String,
    banner: String,

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

    // Channels to communicate with the Help thread
    pub help_tx: Option<Sender<HelpRequest>>,
    pub help_rx: Option<Receiver<HelpReply>>,

    /// Event channel for notifying the LSP. In principle, could be a Jupyter comm.
    lsp_events_tx: Option<TokioUnboundedSender<Event>>,

    dap: RMainDap,

    /// Whether or not R itself is actively busy.
    /// This does not represent the busy state of the kernel.
    pub is_busy: bool,

    /// The `show.error.messages` global option is set to `TRUE` whenever
    /// we get in the browser. We save the previous value here and restore
    /// it the next time we see a non-browser prompt.
    old_show_error_messages: Option<bool>,
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
    /// inputs.  This always corresponds to `getOption("continue"). We send
    /// it to frontends along with `prompt` because some frontends such as
    /// Positron do not send incomplete inputs to Ark and take charge of
    /// continuation prompts themselves.
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
    ) -> Self {
        Self {
            initializing: true,
            r_request_rx,
            comm_manager_tx,
            stdin_request_tx,
            stdin_reply_rx,
            iopub_tx,
            kernel_init_tx,
            active_request: None,
            execution_count: 0,
            stdout: String::new(),
            stderr: String::new(),
            banner: String::new(),
            kernel,
            error_occurred: false,
            error_message: String::new(),
            error_traceback: Vec::new(),
            help_tx: None,
            help_rx: None,
            lsp_events_tx: None,
            dap: RMainDap::new(dap),
            is_busy: false,
            old_show_error_messages: None,
            tasks_interrupt_rx,
            tasks_idle_rx,
            pending_futures: HashMap::new(),
        }
    }

    /// Access a reference to the singleton instance of this struct
    ///
    /// SAFETY: Accesses must occur after `start_r()` initializes it, and must
    /// occur on the main R thread.
    pub fn get() -> &'static Self {
        RMain::get_mut()
    }

    /// Indicate whether RMain has been created and is initialized.
    pub fn initialized() -> bool {
        unsafe {
            match R_MAIN {
                Some(ref main) => !main.initializing,
                None => false,
            }
        }
    }

    /// Access a mutable reference to the singleton instance of this struct
    ///
    /// SAFETY: Accesses must occur after `start_r()` initializes it, and must
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

    pub fn on_main_thread() -> bool {
        let thread = std::thread::current();
        thread.id() == unsafe { R_MAIN_THREAD_ID.unwrap() }
    }

    /// Completes the kernel's initialization
    pub fn complete_initialization(&mut self, prompt_info: &PromptInfo) {
        if self.initializing {
            let version = unsafe {
                let version = Rf_findVarInFrame(R_BaseNamespace, r_symbol!("R.version.string"));
                RObject::new(version).to::<String>().unwrap()
            };

            let kernel_info = KernelInfo {
                version: version.clone(),
                banner: self.banner.clone(),
                input_prompt: Some(prompt_info.input_prompt.clone()),
                continuation_prompt: Some(prompt_info.continuation_prompt.clone()),
            };

            debug!("Sending kernel info: {}", version);
            self.kernel_init_tx.broadcast(kernel_info);
            self.initializing = false;
        } else {
            warn!("Initialization already complete!");
        }
    }

    /// Provides read-only access to `iopub_tx`
    pub fn get_iopub_tx(&self) -> &Sender<IOPubMessage> {
        &self.iopub_tx
    }

    fn init_execute_request(&mut self, req: &ExecuteRequest) -> (ConsoleInput, u32) {
        // Initialize stdout, stderr
        self.stdout = String::new();
        self.stderr = String::new();

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
                warn!(
                    "Could not broadcast execution input {} to all frontends: {}",
                    self.execution_count, err
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
        let info = Self::prompt_info(prompt);
        debug!("R prompt: {}", info.input_prompt);

        INIT_KERNEL.call_once(|| {
            self.complete_initialization(&info);

            trace!(
                "Got initial R prompt '{}', ready for execution requests",
                info.input_prompt
            );
        });

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
            let n = CString::new("n\n").unwrap();
            unsafe {
                libc::strcpy(buf as *mut c_char, n.as_ptr());
            }
            return ConsoleResult::NewInput;
        }

        if let Some(req) = &self.active_request {
            if info.input_request {
                // Request input. We'll wait for a reply in the `select!` below.
                self.request_input(req.orig.clone(), info.input_prompt.to_string());

                // Note that since we're here due to `readline()` or similar, we
                // preserve the current active request. While we are requesting
                // an input and waiting for the reply, the outer
                // `execute_request` remains active and the shell remains busy.
            } else {
                // We got a prompt request marking the end of the previous
                // execution. We can now send a reply to unblock the active Shell
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
                let kernel = self.kernel.lock().unwrap();
                kernel.send_ui_event(event);

                // Let frontend know the last request is complete. This turns us
                // back to Idle.
                self.reply_execute_request(req, info.clone());

                // Clear active request. This doesn't matter if we return here
                // after receiving an `ExecuteCode` request (as
                // `self.active_request` will be set to a fresh request), but
                // we might also return here after an interrupt.
                self.active_request = None;
            }
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
            // Calling handlers don't currently reach inside the
            // debugger. So we temporarily reenable the
            // `show.error.messages` option to let error messages
            // stream to stderr.
            if let None = self.old_show_error_messages {
                let old = r_poke_option_show_error_messages(true);
                self.old_show_error_messages = Some(old);
            }

            match self.dap.stack_info() {
                Ok(stack) => {
                    self.dap.start_debug(stack);
                },
                Err(err) => error!("ReadConsole: Can't get stack info: {err}"),
            };
        } else {
            // We've left the `browser()` state, so we can disable the
            // `show.error.messages` option again to let our global handler
            // capture error messages as before.
            if let Some(old) = self.old_show_error_messages {
                r_poke_option_show_error_messages(old);
                self.old_show_error_messages = None;
            }

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
                    self.handle_task_concurrent(task.unwrap());
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
        trace!("prompt_info(): n_frame = '{}'", n_frame);

        let prompt_slice = unsafe { CStr::from_ptr(prompt_c) };
        let prompt = prompt_slice.to_string_lossy().into_owned();

        // Detect browser prompts by inspecting the `RDEBUG` flag of each
        // frame on the stack. If ANY of the frames are marked with `RDEBUG`,
        // then we assume we are in a debug state. We can't just check the
        // last frame, as sometimes frames are pushed onto the stack by lazy
        // evaluation of arguments or `tryCatch()` that aren't debug frames,
        // but we don't want to exit the debugger when we hit these, as R is
        // still inside a browser state. Should also handle cases like `debug(readline)`
        // followed by `n`.
        // https://github.com/posit-dev/positron/issues/2310
        let frames = RObject::from(harp::session::r_sys_frames().unwrap());
        let browser = r_pairlist_any(frames.sexp, |frame| {
            harp::session::r_env_is_browsed(frame).unwrap()
        });

        // If there are frames on the stack and we're not in a browser prompt,
        // this means some user code is requesting input, e.g. via `readline()`
        let user_request = !browser && n_frame > 0;

        // The request is incomplete if we see the continue prompt, except if
        // we're in a user request, e.g. `readline("+ ")`
        let continuation_prompt = unsafe { r_get_option::<String>("continue").unwrap() };
        let incomplete = !user_request && prompt == continuation_prompt;

        if incomplete {
            trace!("Got R prompt '{}', marking request incomplete", prompt);
        } else if user_request {
            trace!("Got R prompt '{}', asking user for input", prompt);
        }

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

                Self::on_console_input(buf, buflen, code);
                Some(ConsoleResult::NewInput)
            },
            ConsoleInput::EOF => Some(ConsoleResult::Disconnected),
        }
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
                Self::on_console_input(buf, buflen, input);
                ConsoleResult::NewInput
            },
            Err(err) => ConsoleResult::Error(err),
        }
    }

    /// Handle a concurrent (non idle) task.
    ///
    /// Wrapper around `handle_task()` that does some extra logging to record
    /// how long a task waited before being picked up by the R or ReadConsole
    /// event loop.
    fn handle_task_concurrent(&mut self, mut task: RTask) {
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

        self.handle_task(task)
    }

    fn handle_task(&mut self, task: RTask) {
        // Background tasks can't take any user input, so we set R_Interactive
        // to 0 to prevent `readline()` from blocking the task.
        let _interactive = harp::raii::RLocalInteractive::new(false);

        let start_info = match task {
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
        };

        if let Some(info) = start_info {
            if info.elapsed() > std::time::Duration::from_millis(50) {
                let _s = info.span.enter();
                log::info!("task took {} milliseconds.", info.elapsed().as_millis());
            }
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

    /// Copy console input into R's internal input buffer
    ///
    /// Supposedly `buflen` is "the maximum length, in bytes, including the
    /// terminator". In practice it seems like R adds 1 extra byte on top of
    /// this when allocating the buffer, but we don't abuse that.
    /// https://github.com/wch/r-source/blob/20c9590fd05c54dba6c9a1047fb0ba7822ba8ba2/src/include/Defn.h#L1863-L1865
    ///
    /// In the case of receiving too much input, we simply trim the string and
    /// log the issue, executing the rest. Ideally the front end will break up
    /// large inputs, preventing this from being necessary. The important thing
    /// is to avoid a crash, and it seems that we need to copy something into
    /// R's buffer to keep the REPL in a good state.
    /// https://github.com/posit-dev/positron/issues/1326#issuecomment-1745389921
    fn on_console_input(buf: *mut c_uchar, buflen: c_int, mut input: String) {
        let buflen = buflen as usize;

        if buflen < 2 {
            // Pathological case. A user wouldn't be able to do anything useful anyways.
            panic!("Console input `buflen` must be >=2.");
        }

        // Leave room for final `\n` and `\0` terminator
        let buflen = buflen - 2;

        if input.len() > buflen {
            log::error!("Console input too large for buffer, writing R error.");
            input = Self::buffer_overflow_call();
        }

        // Push `\n`
        input.push_str("\n");

        // Push `\0` (automatically, as it converts to a C string)
        let input = CString::new(input).unwrap();

        unsafe {
            libc::strcpy(buf as *mut c_char, input.as_ptr());
        }
    }

    // Temporary patch for https://github.com/posit-dev/positron/issues/2675.
    // We write an informative `stop()` call rather than the user's actual input.
    fn buffer_overflow_call() -> String {
        let message = r#"
Can't pass console input on to R, it exceeds R's internal console buffer size.
This is a Positron limitation we plan to fix. In the meantime, you can:
- Break the command you sent to the console into smaller chunks, if possible.
- Otherwise, send the whole script to the console using `source()`.
        "#;

        let message = message.trim();
        let message = format!("stop(\"{message}\")");

        message
    }

    // Reply to the previously active request. The current prompt type and
    // whether an error has occurred defines the response kind.
    fn reply_execute_request(&self, req: &ActiveReadConsoleRequest, prompt_info: PromptInfo) {
        let prompt = prompt_info.input_prompt;

        let reply = if prompt_info.incomplete {
            trace!("Got prompt {} signaling incomplete request", prompt);
            new_incomplete_response(&req.request, req.exec_count)
        } else if prompt_info.input_request {
            unreachable!();
        } else {
            trace!("Got R prompt '{}', completing execution", prompt);
            peek_execute_response(req.exec_count)
        };
        req.response_tx.send(reply).unwrap();
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
    fn write_console(&mut self, buf: *const c_char, _buflen: i32, otype: i32) {
        let content = match console_to_utf8(buf) {
            Ok(content) => content,
            Err(err) => panic!("Failed to read from R buffer: {err:?}"),
        };

        // To capture the current `debug: <call>` output, for use in the debugger's
        // match based fallback
        self.dap.handle_stdout(&content);

        let stream = if otype == 0 {
            Stream::Stdout
        } else {
            Stream::Stderr
        };

        if self.initializing {
            // During init, consider all output to be part of the startup banner
            self.banner.push_str(&content);
            return;
        }

        // If active execution request is silent don't broadcast
        // any output
        if let Some(ref req) = self.active_request {
            if req.request.silent {
                return;
            }
        }

        let buffer = match stream {
            Stream::Stdout => &mut self.stdout,
            Stream::Stderr => &mut self.stderr,
        };

        // Append content to buffer.
        buffer.push_str(&content);

        // Stream output via the IOPub channel.
        let message = IOPubMessage::Stream(StreamOutput {
            name: stream,
            text: content,
        });

        unwrap!(self.iopub_tx.send(message), Err(error) => {
            log::error!("{}", error);
        });
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
                self.handle_task_concurrent(task);
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

// Gets response data from R state
fn peek_execute_response(exec_count: u32) -> ExecuteResponse {
    let main = RMain::get_mut();

    // Save and reset error occurred flag
    let error_occurred = main.error_occurred;
    main.error_occurred = false;

    // Error handlers are not called on stack overflow so the error flag
    // isn't set. Instead we detect stack overflows by peeking at the error
    // buffer. The message is explicitly not translated to save stack space
    // so the matching should be reliable.
    let err_buf = geterrmessage();
    let stack_overflow_occurred = RE_STACK_OVERFLOW.is_match(&err_buf);

    // Reset error buffer so we don't display this message again
    if stack_overflow_occurred {
        let _ = RFunction::new("base", "stop").call();
    }

    // Send the reply to the frontend
    if error_occurred || stack_overflow_occurred {
        // We don't fill out `ename` with anything meaningful because typically
        // R errors don't have names. We could consider using the condition class
        // here, which r-lib/tidyverse packages have been using more heavily.
        let exception = if error_occurred {
            Exception {
                ename: String::from(""),
                evalue: main.error_message.clone(),
                traceback: main.error_traceback.clone(),
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

        log::info!("An R error occurred: {}", exception.evalue);

        main.iopub_tx
            .send(IOPubMessage::ExecuteError(ExecuteError {
                exception: exception.clone(),
            }))
            .or_log_warning(&format!("Could not publish error {} on iopub", exec_count));

        new_execute_error_response(exception, exec_count)
    } else {
        // TODO: Implement rich printing of certain outputs.
        // Will we need something similar to the RStudio model,
        // where we implement custom print() methods? Or can
        // we make the stub below behave sensibly even when
        // streaming R output?
        let mut data = serde_json::Map::new();
        data.insert("text/plain".to_string(), json!(""));

        // Include HTML representation of data.frame
        unsafe {
            let value = Rf_findVarInFrame(R_GlobalEnv, r_symbol!(".Last.value"));
            if r_is_data_frame(value) {
                match to_html(value) {
                    Ok(html) => data.insert("text/html".to_string(), json!(html)),
                    Err(error) => {
                        error!("{:?}", error);
                        None
                    },
                };
            }
        }

        main.iopub_tx
            .send(IOPubMessage::ExecuteResult(ExecuteResult {
                execution_count: exec_count,
                data: serde_json::Value::Object(data),
                metadata: json!({}),
            }))
            .or_log_warning(&format!(
                "Could not publish result of statement {} on iopub",
                exec_count
            ));

        new_execute_response(exec_count)
    }
}

fn new_execute_response(exec_count: u32) -> ExecuteResponse {
    ExecuteResponse::Reply(ExecuteReply {
        status: Status::Ok,
        execution_count: exec_count,
        user_expressions: json!({}),
    })
}
fn new_execute_error_response(exception: Exception, exec_count: u32) -> ExecuteResponse {
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

            {
                let err_cstring = new_cstring(format!("{err:?}"));
                unsafe {
                    ERROR_BUF = Some(
                        CString::new(format!(
                            "Error while reading input: {}",
                            err_cstring.into_string().unwrap()
                        ))
                        .unwrap(),
                    );
                }
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
    let main = RMain::get_mut();
    main.write_console(buf, buflen, otype);
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
pub unsafe extern "C" fn r_polled_events() {
    let main = RMain::get_mut();
    main.polled_events();
}
