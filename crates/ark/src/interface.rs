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

use std::collections::VecDeque;
use std::ffi::*;
use std::os::raw::c_uchar;
use std::result::Result::Ok;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Once;
use std::time::Duration;

use amalthea::comm::base_comm::JsonRpcReply;
use amalthea::comm::event::CommManagerEvent;
use amalthea::comm::frontend_comm::BusyParams;
use amalthea::comm::frontend_comm::FrontendEvent;
use amalthea::comm::frontend_comm::FrontendFrontendRpcRequest;
use amalthea::comm::frontend_comm::PromptStateParams;
use amalthea::comm::frontend_comm::ShowMessageParams;
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
use amalthea::wire::input_request::CommRequest;
use amalthea::wire::input_request::InputRequest;
use amalthea::wire::input_request::ShellInputRequest;
use amalthea::wire::input_request::StdInRpcReply;
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
use crossbeam::channel::TryRecvError;
use crossbeam::select;
use harp::exec::geterrmessage;
use harp::exec::r_check_stack;
use harp::exec::r_sandbox;
use harp::exec::r_source;
use harp::exec::RFunction;
use harp::exec::RFunctionExt;
use harp::exec::RSandboxScope;
use harp::line_ending::convert_line_endings;
use harp::line_ending::LineEnding;
use harp::object::RObject;
use harp::r_symbol;
use harp::routines::r_register_routines;
use harp::session::r_traceback;
use harp::utils::r_get_option;
use harp::utils::r_is_data_frame;
use harp::utils::r_poke_option_show_error_messages;
use harp::R_MAIN_THREAD_ID;
use libR_shim::R_BaseNamespace;
use libR_shim::R_GlobalEnv;
use libR_shim::R_ProcessEvents;
use libR_shim::R_RunPendingFinalizers;
use libR_shim::Rf_error;
use libR_shim::Rf_findVarInFrame;
use libR_shim::Rf_onintr;
use libR_shim::SEXP;
use log::*;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::json;
use stdext::result::ResultOrLog;
use stdext::*;
use tokio::runtime::Runtime;
use tower_lsp::Client;

use crate::dap::dap::DapBackendEvent;
use crate::dap::Dap;
use crate::errors;
use crate::help::message::HelpReply;
use crate::help::message::HelpRequest;
use crate::kernel::Kernel;
use crate::lsp::events::EVENTS;
use crate::modules;
use crate::plots::graphics_device;
use crate::r_task;
use crate::r_task::RTaskMain;
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
    input_reply_rx: Receiver<amalthea::Result<InputReply>>,
    iopub_tx: Sender<IOPubMessage>,
    kernel_init_tx: Bus<KernelInfo>,
    lsp_runtime: Arc<Runtime>,
    lsp_client: Client,
    dap: Arc<Mutex<Dap>>,
) {
    // Initialize global state (ensure we only do this once!)
    INIT.call_once(|| unsafe {
        R_MAIN_THREAD_ID = Some(std::thread::current().id());

        // Channels to send/receive tasks from auxiliary threads via `r_task()`
        let (tasks_tx, tasks_rx) = unbounded::<RTaskMain>();

        r_task::initialize(tasks_tx);

        R_MAIN = Some(RMain::new(
            kernel_mutex,
            tasks_rx,
            comm_manager_tx,
            r_request_rx,
            stdin_request_tx,
            input_reply_rx,
            iopub_tx,
            kernel_init_tx,
            lsp_runtime,
            lsp_client,
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

    crate::sys::interface::setup_r(args);

    unsafe {
        // Optionally run a user specified R startup script
        if let Some(file) = &startup_file {
            r_source(file).or_log_error(&format!("Failed to source startup file '{file}' due to"));
        }

        // Initialize harp.
        harp::initialize();

        // Register embedded routines
        r_register_routines();

        // Initialize support functions (after routine registration)
        modules::initialize(false).unwrap();

        // Register all hooks once all modules have been imported
        let hook_result = RFunction::from(".ps.register_all_hooks").call();
        if let Err(err) = hook_result {
            warn!("Error registering some hooks: {}", err);
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
    input_reply_rx: Receiver<amalthea::Result<InputReply>>,

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

    /// Channel to receive tasks from `r_task()`
    tasks_rx: Receiver<RTaskMain>,
    pending_tasks: VecDeque<RTaskMain>,

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

    // LSP tokio runtime used to spawn LSP tasks on the executor and the
    // corresponding client used to send LSP requests to the frontend.
    // Used by R callbacks, like `ps_editor()` for `utils::file.edit()`.
    lsp_runtime: Arc<Runtime>,
    lsp_client: Client,

    dap: Arc<Mutex<Dap>>,
    is_debugging: bool,

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
        tasks_rx: Receiver<RTaskMain>,
        comm_manager_tx: Sender<CommManagerEvent>,
        r_request_rx: Receiver<RRequest>,
        stdin_request_tx: Sender<StdInRequest>,
        input_reply_rx: Receiver<amalthea::Result<InputReply>>,
        iopub_tx: Sender<IOPubMessage>,
        kernel_init_tx: Bus<KernelInfo>,
        lsp_runtime: Arc<Runtime>,
        lsp_client: Client,
        dap: Arc<Mutex<Dap>>,
    ) -> Self {
        Self {
            initializing: true,
            r_request_rx,
            comm_manager_tx,
            stdin_request_tx,
            input_reply_rx,
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
            lsp_runtime,
            lsp_client,
            dap,
            is_debugging: false,
            is_busy: false,
            old_show_error_messages: None,
            tasks_rx,
            pending_tasks: VecDeque::new(),
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
            match &R_MAIN {
                Some(main) => !main.initializing,
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
                    "Could not broadcast execution input {} to all front ends: {}",
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
                let event = FrontendEvent::PromptState(PromptStateParams {
                    input_prompt: info.input_prompt.clone(),
                    continuation_prompt: info.continuation_prompt.clone(),
                });
                let kernel = self.kernel.lock().unwrap();
                kernel.send_frontend_event(event);

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

            let mut dap = self.dap.lock().unwrap();
            match harp::session::r_stack_info() {
                Ok(stack) => {
                    self.is_debugging = true;
                    dap.start_debug(stack)
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

            if self.is_debugging {
                // Terminate debugging session
                let mut dap = self.dap.lock().unwrap();
                dap.stop_debug();
                self.is_debugging = false;
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

            // Yield to auxiliary threads and to the R event loop
            self.yield_to_tasks();
            unsafe { Self::process_events() };

            // FIXME: Race between interrupt and new code request. To fix
            // this, we could manage the Shell and Control sockets on the
            // common message event thread. The Control messages would need
            // to be handled in a blocking way to ensure subscribers are
            // notified before the next incoming message is processed.

            select! {
                // Wait for an execution request from the front end.
                recv(self.r_request_rx) -> req => {
                    let req = unwrap!(req, Err(_) => {
                        // The channel is disconnected and empty
                        return ConsoleResult::Disconnected;
                    });

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
                            if !self.is_debugging {
                                continue;
                            }

                            // Translate requests from the debugger frontend to actual inputs for
                            // the debug interpreter
                            ConsoleInput::Input(debug_request_command(cmd))
                        },
                    };

                    // Clear error flag
                    self.error_occurred = false;

                    return match input {
                        ConsoleInput::Input(code) => {
                            // Handle commands for the debug interpreter
                            if self.is_debugging {
                                let continue_cmds = vec!["n", "f", "c", "cont"];
                                if continue_cmds.contains(&&code[..]) {
                                    self.send_dap(DapBackendEvent::Continued);
                                }
                            }

                            Self::on_console_input(buf, buflen, code);
                            ConsoleResult::NewInput
                        },
                        ConsoleInput::EOF => ConsoleResult::Disconnected,
                    }
                }

                recv(self.input_reply_rx) -> chan_result => {
                    // StdIn must remain alive
                    let result = chan_result.unwrap();

                    match result {
                        Ok(input) => {
                            let input = convert_line_endings(&input.value, LineEnding::Posix);

                            Self::on_console_input(buf, buflen, input);
                            return ConsoleResult::NewInput;
                        },
                        Err(err) => {
                            return ConsoleResult::Error(err);
                        }
                    }
                }

                // A task woke us up, start next loop tick to yield to it
                recv(self.tasks_rx) -> task => {
                    if let Ok(task) = task {
                        self.pending_tasks.push_back(task);
                    }
                    continue;
                }

                // Wait with a timeout. Necessary because we need to
                // pump the event loop while waiting for console input.
                //
                // Alternatively, we could try to figure out the file
                // descriptors that R has open and select() on those for
                // available data?
                default(Duration::from_millis(200)) => {
                    continue;
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

        // Detect browser prompts by inspecting the `RDEBUG` flag of the
        // last frame on the stack. This is not 100% infallible, for
        // instance `debug(readline)` followed by `n` will instantiate a
        // user request prompt that will look like a browser prompt
        // according to this heuristic. However it has the advantage of
        // correctly detecting that continue prompts are top-level browser
        // prompts in case of incomplete inputs within `browser()`.
        let frame = harp::session::r_sys_frame(n_frame).unwrap();
        let browser = harp::session::r_env_is_browsed(frame).unwrap();

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

    fn on_console_input(buf: *mut c_uchar, buflen: c_int, mut input: String) {
        // TODO: What if the input is too large for the buffer?
        input.push_str("\n");
        if input.len() > buflen as usize {
            info!("Error: input too large for buffer.");
            return;
        }

        let src = CString::new(input).unwrap();
        unsafe {
            libc::strcpy(buf as *mut c_char, src.as_ptr());
        }
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
        let event = FrontendEvent::Busy(BusyParams { busy: self.is_busy });

        // Wait for a lock on the kernel and have it deliver the event to
        // the front end
        let kernel = self.kernel.lock().unwrap();
        kernel.send_frontend_event(event);
    }

    /// Invoked by R to show a message to the user.
    fn show_message(&self, buf: *const c_char) {
        let message = unsafe { CStr::from_ptr(buf) };

        // Create an event representing the message
        let event = FrontendEvent::ShowMessage(ShowMessageParams {
            message: message.to_str().unwrap().to_string(),
        });

        // Wait for a lock on the kernel and have the kernel deliver the
        // event to the front end
        let kernel = self.kernel.lock().unwrap();
        kernel.send_frontend_event(event);
    }

    /// Invoked by the R event loop
    fn polled_events(&mut self) {
        let _scope = RSandboxScope::new();
        self.yield_to_tasks();
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

    fn yield_to_tasks(&mut self) {
        // Skip task if we don't have 128KB of stack space available.  This
        // is 1/8th of the typical Windows stack space (1MB, whereas macOS
        // and Linux have 8MB).
        if let Err(_) = r_check_stack(Some(128 * 1024)) {
            return;
        }

        loop {
            match self.tasks_rx.try_recv() {
                Ok(task) => self.pending_tasks.push_back(task),
                Err(TryRecvError::Empty) => break,
                Err(err) => log::error!("{err:}"),
            }
        }

        // Run pending tasks but yield back to R after max 3 tasks
        for _ in 0..3 {
            if let Some(mut task) = self.pending_tasks.pop_front() {
                log::info!(
                    "Yielding to task - {} more task(s) remaining",
                    self.pending_tasks.len()
                );
                task.fulfill();
            } else {
                return;
            }
        }
    }

    fn send_dap(&self, event: DapBackendEvent) {
        let dap = self.dap.lock().unwrap();
        if let Some(tx) = &dap.backend_events_tx {
            log_error!(tx.send(event));
        }
    }

    pub fn get_comm_manager_tx(&self) -> &Sender<CommManagerEvent> {
        // Read only access to `comm_manager_tx`
        &self.comm_manager_tx
    }

    pub fn get_kernel(&self) -> &Arc<Mutex<Kernel>> {
        &self.kernel
    }

    pub fn get_lsp_runtime(&self) -> &Arc<Runtime> {
        &self.lsp_runtime
    }

    pub fn get_lsp_client(&self) -> &Client {
        &self.lsp_client
    }

    pub fn call_frontend_method(
        &self,
        request: FrontendFrontendRpcRequest,
    ) -> anyhow::Result<RObject> {
        // If an interrupt was signalled, returns `NULL`. This should not be
        // visible to the caller since `r_unwrap()` (called e.g. by
        // `harp::register`) will trigger an interrupt jump right away.
        match self.call_frontend_method_safe(request) {
            Some(result) => result,
            None => Ok(RObject::null()),
        }
    }

    // If returns `None`, it means the request was interrupted and we need to
    // propagate the interrupt to R
    fn call_frontend_method_safe(
        &self,
        request: FrontendFrontendRpcRequest,
    ) -> Option<anyhow::Result<RObject>> {
        log::trace!("Calling frontend method '{request:?}'");
        let (response_tx, response_rx) = bounded(1);

        // NOTE: Probably simpler to share the originator through a mutex
        // than pass it around
        let orig = if let Some(req) = &self.active_request {
            if let Some(orig) = &req.orig {
                orig
            } else {
                return Some(Err(anyhow::anyhow!("Error: No active originator")));
            }
        } else {
            return Some(Err(anyhow::anyhow!("Error: No active request")));
        };

        let request = CommRequest {
            originator: Some(orig.clone()),
            response_tx,
            request,
        };

        {
            let kernel = self.kernel.lock().unwrap();
            kernel.send_frontend_request(request);
        }

        // Create request and block for response
        let response = response_rx.recv().unwrap();

        log::trace!("Got response from frontend method: {response:?}");

        match response {
            StdInRpcReply::Response(response) => match response {
                JsonRpcReply::Result(response) => {
                    Some(RObject::try_from(response.result).map_err(|err| anyhow::anyhow!("{err}")))
                },
                JsonRpcReply::Error(response) => Some(Err(anyhow::anyhow!(
                    "While calling frontend method':\n\
                     {}",
                    response.error.message
                ))),
            },
            StdInRpcReply::Interrupt => None,
        }
    }
}

/// Report an incomplete request to the front end
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

    // Send the reply to the front end
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
        r_task(|| unsafe {
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
        });

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
