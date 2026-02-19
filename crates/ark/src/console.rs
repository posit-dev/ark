//
// console.rs
//
// Copyright (C) 2023-2026 Posit Software, PBC. All rights reserved.
//
//

// All code in this file runs synchronously with R. We store the global
// state inside of a global `CONSOLE` singleton that implements `Console`.
// The frontend methods called by R are forwarded to the corresponding
// `Console` methods via `CONSOLE`.

use std::cell::Cell;
use std::cell::RefCell;
use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::ffi::*;
use std::os::raw::c_uchar;
use std::result::Result::Ok;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::Poll;
use std::time::Duration;

use amalthea::comm::base_comm::JsonRpcReply;
use amalthea::comm::event::CommEvent;
use amalthea::comm::ui_comm::ui_frontend_reply_from_value;
use amalthea::comm::ui_comm::BusyParams;
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
use amalthea::wire::execute_request::CodeLocation;
use amalthea::wire::execute_request::ExecuteRequest;
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
use crossbeam::channel::Receiver;
use crossbeam::channel::Sender;
use harp::command::r_command;
use harp::command::r_home_setup;
use harp::environment::r_ns_env;
use harp::environment::Environment;
use harp::environment::R_ENVS;
use harp::exec::exec_with_cleanup;
use harp::exec::r_check_stack;
use harp::exec::r_peek_error_buffer;
use harp::exec::r_sandbox;
use harp::exec::with_calling_error_handler;
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
use harp::srcref::get_srcref_list;
use harp::srcref::srcref_list_get;
use harp::srcref::SrcFile;
use harp::utils::r_is_data_frame;
use harp::utils::r_typeof;
use harp::CONSOLE_THREAD_ID;
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
use stdext::result::ResultExt;
use stdext::*;
use tokio::sync::mpsc::UnboundedReceiver as AsyncUnboundedReceiver;
use url::Url;
use uuid::Uuid;

use crate::console_annotate::annotate_input;
use crate::console_debug::FrameInfoId;
use crate::dap::dap::Breakpoint;
use crate::dap::Dap;
use crate::errors::stack_overflow_occurred;
use crate::help::message::HelpEvent;
use crate::help::r_help::RHelp;
use crate::lsp::events::EVENTS;
use crate::lsp::main_loop::DidCloseVirtualDocumentParams;
use crate::lsp::main_loop::DidOpenVirtualDocumentParams;
use crate::lsp::main_loop::Event;
use crate::lsp::main_loop::KernelNotification;
use crate::lsp::main_loop::TokioUnboundedSender;
use crate::lsp::state_handlers::ConsoleInputs;
use crate::modules;
use crate::modules::ARK_ENVS;
use crate::plots::graphics_device;
use crate::plots::graphics_device::GraphicsDeviceNotification;
use crate::r_task;
use crate::r_task::BoxFuture;
use crate::r_task::RTask;
use crate::r_task::RTaskStartInfo;
use crate::r_task::RTaskStatus;
use crate::repos::apply_default_repos;
use crate::repos::DefaultRepos;
use crate::request::debug_request_command;
use crate::request::KernelRequest;
use crate::request::RRequest;
use crate::signals::initialize_signal_handlers;
use crate::signals::interrupts_pending;
use crate::signals::set_interrupts_pending;
use crate::srcref::ns_populate_srcref;
use crate::srcref::resource_loaded_namespaces;
use crate::startup;
use crate::sys::console::console_to_utf8;
use crate::ui::UiCommMessage;
use crate::ui::UiCommSender;
use crate::url::ExtUrl;

static RE_DEBUG_PROMPT: Lazy<Regex> = Lazy::new(|| Regex::new(r"Browse\[\d+\]").unwrap());

/// All debug commands as documented in `?browser`
const DEBUG_COMMANDS: &[&str] = &["c", "cont", "f", "help", "n", "s", "where", "r", "Q"];

// Debug commands that exit the current browser: `n`, `f`, `c`, `cont` continue
// execution past the current prompt, `Q` exits all nested browsers entirely.
// These are not transient evals: they represent deliberate debugger navigation.
const DEBUG_COMMANDS_CONTINUE: &[&str] = &["n", "f", "c", "cont", "Q"];

/// An enum representing the different modes in which the R session can run.
#[derive(PartialEq, Clone, Copy)]
pub enum SessionMode {
    /// A session with an interactive console (REPL), such as in Positron.
    Console,

    /// A session in a Jupyter or Jupyter-like notebook.
    Notebook,

    /// A background session, typically not connected to any UI.
    Background,
}

#[derive(Clone, Debug)]
pub enum DebugCallText {
    None,
    Capturing(String, DebugCallTextKind),
    Finalized(String, DebugCallTextKind),
}

#[derive(Clone, Copy, Debug)]
pub enum DebugCallTextKind {
    Debug,
    DebugAt,
}

#[derive(Debug, Clone)]
pub enum DebugStoppedReason {
    Step,
    Pause,
    Condition { class: String, message: String },
}

/// Notifications from other components (e.g., LSP) to the Console
#[derive(Debug)]
pub enum ConsoleNotification {
    /// Notification that a document has changed, requiring breakpoint invalidation.
    DidChangeDocument(Url),
}

// --- Globals ---
// These values must be global in order for them to be accessible from R
// callbacks, which do not have a facility for passing or returning context.

/// Used to wait for complete R startup in `Console::wait_initialized()` or
/// check for it in `Console::is_initialized()`.
///
/// We use the `once_cell` crate for init synchronisation because the stdlib
/// equivalent `std::sync::Once` does not have a `wait()` method.
static R_INIT: once_cell::sync::OnceCell<()> = once_cell::sync::OnceCell::new();

thread_local! {
    /// The `Console` singleton.
    ///
    /// It is wrapped in an `UnsafeCell` because we currently need to bypass the
    /// borrow checker rules (see https://github.com/posit-dev/ark/issues/663).
    /// The `UnsafeCell` itself is wrapped in a `RefCell` because that's the
    /// only way to get a `set()` method on the thread-local storage key and
    /// bypass the lazy initializer (which panics for other threads).
    pub static CONSOLE: RefCell<UnsafeCell<Console>> = panic!("Must access `CONSOLE` from the R thread");
}

pub struct Console {
    kernel_request_rx: Receiver<KernelRequest>,

    /// Whether we are running in Console, Notebook, or Background mode.
    session_mode: SessionMode,

    /// Channel used to send along messages relayed on the open comms.
    comm_event_tx: Sender<CommEvent>,

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

    /// Active request passed to `ReadConsole()`. Contains reply channel
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
    tasks_idle_any_rx: Receiver<RTask>,
    pending_futures: HashMap<Uuid, (BoxFuture<'static, ()>, RTaskStartInfo)>,

    /// Channel to communicate requests and events to the frontend
    /// by forwarding them through the UI comm. Optional, and really Positron specific.
    ui_comm_tx: Option<UiCommSender>,

    /// Error captured by our global condition handler during the last iteration
    /// of the REPL.
    pub(crate) last_error: Option<Exception>,

    /// Channel to communicate with the Help thread
    help_event_tx: Option<Sender<HelpEvent>>,
    /// R help port
    help_port: Option<u16>,

    /// Event channel for notifying the LSP. In principle, could be a Jupyter comm.
    lsp_events_tx: Option<TokioUnboundedSender<Event>>,

    /// The kernel's copy of virtual documents to notify the LSP about when the LSP
    /// initially connects and after an LSP restart.
    lsp_virtual_documents: HashMap<String, String>,

    pub positron_ns: Option<RObject>,

    pending_inputs: Option<PendingInputs>,

    /// Banner output accumulated during startup, but set to `None` after we complete
    /// the initialization procedure and forward the banner on
    banner: Option<String>,

    /// Raw error buffer provided to `Rf_error()` when throwing `r_read_console()` errors.
    /// Stored in `Console` to avoid memory leakage when `Rf_error()` jumps.
    r_error_buffer: Option<CString>,

    /// When `Some`, console output is captured here instead of being sent to IOPub.
    /// Interact with this via `ConsoleOutputCapture` from `start_capture()`.
    pub(crate) captured_output: Option<String>,

    /// Whether the current evaluation is transient within the debug session.
    /// When `true`, the debug session state is preserved: no Continued/Stopped
    /// events are emitted, frame IDs remain valid, and only an Invalidated
    /// event is sent to refresh variables. Set to `true` for console
    /// evaluations (as opposed to step commands like `n`, `c`, `f`).
    /// See https://github.com/posit-dev/positron/issues/3151.
    pub(crate) debug_transient_eval: bool,

    /// Underlying dap state. Shared with the DAP server thread.
    pub(crate) debug_dap: Arc<Mutex<Dap>>,

    /// Whether or not we are currently in a debugging state.
    pub(crate) debug_is_debugging: bool,

    /// The current call emitted by R as `debug: <call-text>`.
    pub(crate) debug_call_text: DebugCallText,

    /// The last known `start_line` for the active context frame.
    pub(crate) debug_last_line: Option<i64>,

    /// The stack of frames we saw the last time we stopped. Used as a mostly
    /// reliable indication of whether we moved since last time.
    pub(crate) debug_last_stack: Vec<FrameInfoId>,

    /// Ever increasing debug session index. Used to create URIs that are only
    /// valid for a single session.
    pub(crate) debug_session_index: u32,

    /// The current frame `id`. Monotonically increasing, unique across all
    /// frames and debug sessions. It's important that each frame gets a unique
    /// ID across the process lifetime so that we can invalidate stale requests.
    pub(crate) debug_current_frame_id: i64,

    /// Reason for entering the debugger. Used to determine which DAP event to send.
    pub(crate) debug_stopped_reason: Option<DebugStoppedReason>,

    /// The frame ID selected by the user in the debugger UI.
    /// When set, console evaluations happen in this frame's environment instead of the current frame.
    /// Resolved to an environment via `debug_dap` state when needed.
    pub(crate) debug_selected_frame_id: Cell<Option<i64>>,

    /// Saved JIT compiler level, to restore after a step-into command.
    /// Step-into disables JIT to prevent stepping into `compiler` internals.
    pub(crate) debug_jit_level: Option<i32>,

    /// Tracks how many nested `r_read_console()` calls are on the stack.
    /// Incremented when entering `r_read_console(),` decremented on exit.
    read_console_depth: Cell<usize>,

    /// Set to true when `r_read_console()` exits via an error longjump. Used to
    /// detect if we need to go return from `r_read_console()` with a dummy
    /// evaluation to reset things like `R_EvalDepth`.
    read_console_threw_error: Cell<bool>,

    /// Set to true when `r_read_console()` exits. Reset to false at the start
    /// of each `r_read_console()` call. Used to detect if `eval()` returned
    /// from a nested REPL (the flag will be true when the evaluation returns).
    /// In these cases, we need to return from `r_read_console()` with a dummy
    /// evaluation to reset things like `R_ConsoleIob`.
    read_console_nested_return: Cell<bool>,

    /// Pending action to perform at the start of the next `r_read_console()` call.
    read_console_pending_action: Cell<ReadConsolePendingAction>,

    /// We've received a Shutdown signal and need to return EOF from all nested
    /// consoles to get R to shut down
    read_console_shutdown: Cell<bool>,

    /// Stack of topmost environments while waiting for input in ReadConsole.
    /// Pushed on entry to `r_read_console()`, popped on exit.
    /// This is a RefCell since we require `get()` for this field and `RObject` isn't `Copy`.
    pub(crate) read_console_env_stack: RefCell<Vec<RObject>>,
}

/// Stack of pending inputs
struct PendingInputs {
    /// EXPRSXP vector of parsed expressions
    exprs: RObject,
    /// List of srcrefs if any, the same length as `exprs`
    srcrefs: Option<RObject>,
    /// Length of `exprs` and `srcrefs`
    len: isize,
    /// Index into the stack
    index: isize,
}

enum ParseResult<T> {
    Success(Option<T>),
    SyntaxError(String),
}

impl PendingInputs {
    pub(crate) fn read(
        code: &str,
        location: Option<CodeLocation>,
        breakpoints: Option<&mut [Breakpoint]>,
    ) -> anyhow::Result<ParseResult<PendingInputs>> {
        let input = if let Some(location) = location {
            match annotate_input(code, location, breakpoints) {
                Ok(annotated_code) => {
                    log::trace!("Annotated code: \n```\n{annotated_code}\n```");
                    harp::ParseInput::SrcFile(&SrcFile::new_virtual_empty_filename(
                        annotated_code.into(),
                    ))
                },
                Err(err) => {
                    log::warn!("{err:?}");
                    harp::ParseInput::Text(code)
                },
            }
        } else if harp::get_option_bool("keep.source") {
            harp::ParseInput::SrcFile(&SrcFile::new_virtual_empty_filename(code.into()))
        } else {
            harp::ParseInput::Text(code)
        };

        let status = match harp::parse_status(&input) {
            Err(err) => {
                // Failed to even attempt to parse the input, something is seriously wrong
                return Ok(ParseResult::SyntaxError(format!("{err}")));
            },
            Ok(status) => status,
        };

        // - Incomplete inputs put R into a state where it expects more input that will never come, so we
        //   immediately reject them. Positron should never send us these, but Jupyter Notebooks may.
        // - Complete statements are obviously fine.
        // - Syntax errors will get bubbled up as R errors via an `ConsoleResult::Error`.
        let exprs = match status {
            harp::ParseResult::Complete(exprs) => exprs,
            harp::ParseResult::Incomplete => {
                return Ok(ParseResult::SyntaxError(format!(
                    "Can't parse incomplete input"
                )));
            },
            harp::ParseResult::SyntaxError { message, .. } => {
                return Ok(ParseResult::SyntaxError(format!("Syntax error: {message}")));
            },
        };

        let srcrefs = get_srcref_list(exprs.sexp);

        let len = exprs.length();
        let index = 0;

        if len == 0 {
            return Ok(ParseResult::Success(None));
        }

        Ok(ParseResult::Success(Some(Self {
            exprs,
            srcrefs,
            len,
            index,
        })))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.index >= self.len
    }

    pub(crate) fn pop(&mut self) -> Option<PendingInput> {
        if self.is_empty() {
            return None;
        }

        let expr = RObject::new(harp::list_get(self.exprs.sexp, self.index));

        let srcref = self
            .srcrefs
            .as_ref()
            .map(|xs| srcref_list_get(xs.sexp, self.index))
            .unwrap_or(RObject::null());

        self.index += 1;
        Some(PendingInput { expr, srcref })
    }
}

#[derive(Debug)]
pub(crate) struct PendingInput {
    expr: RObject,
    srcref: RObject,
}

#[derive(Debug, Clone)]
enum ConsoleValue {
    Success(serde_json::Map<String, serde_json::Value>),
    Error(Exception),
}

enum WaitFor {
    InputReply,
    ExecuteRequest,
}

/// Represents the currently active execution request from the frontend. It
/// resolves at the next invocation of the `ReadConsole()` frontend method.
struct ActiveReadConsoleRequest {
    exec_count: u32,
    request: ExecuteRequest,
    originator: Originator,
    reply_tx: Sender<amalthea::Result<ExecuteReply>>,
}

/// Represents kernel metadata (available after the kernel has fully started)
#[derive(Debug, Clone)]
pub struct KernelInfo {
    pub version: String,
    pub banner: String,
    pub input_prompt: Option<String>,
    pub continuation_prompt: Option<String>,
}

/// The kind of prompt we're handling in the REPL.
#[derive(Clone, Debug, PartialEq)]
pub enum PromptKind {
    /// A top-level REPL prompt
    TopLevel,

    /// A `browser()` debugging prompt
    Browser,

    /// A user input request from code, e.g., via `readline()`
    InputRequest,
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
    /// inputs. This always corresponds to `getOption("continue")`. We send
    /// it to frontends along with `prompt` because some frontends such as
    /// Positron do not send incomplete inputs to Ark and take charge of
    /// continuation prompts themselves. For frontends that can send
    /// incomplete inputs to Ark, like Jupyter Notebooks, we immediately
    /// error on them rather than requesting that this be shown.
    continuation_prompt: String,

    /// The kind of prompt we're handling.
    kind: PromptKind,
}

pub enum ConsoleInput {
    EOF,
    Input(String, Option<CodeLocation>),
}

#[derive(Debug)]
pub(crate) enum ConsoleResult {
    NewInput,
    NewPendingInput(PendingInput),
    Interrupt,
    Disconnected,
    Error(String),
}

/// Guard for capturing console output during idle tasks.
///
/// Created via `Console::start_capture()`, which sets `Console::captured_output`
/// to `Some` so that all `write_console` output goes there instead of IOPub.
/// When dropped, restores the previous state and logs any remaining output.
///
/// Use `take()` to retrieve captured output. Can be called multiple times to
/// get output accumulated since the last take.
pub struct ConsoleOutputCapture {
    previous_output: Option<String>,
    connected: bool,
}

impl ConsoleOutputCapture {
    /// Create a dummy capture that doesn't interact with Console.
    /// Used in test contexts where Console is not initialized.
    pub(crate) fn dummy() -> Self {
        Self {
            previous_output: None,
            connected: false,
        }
    }

    /// Take the captured output so far, clearing the buffer.
    /// Can be called multiple times; each call returns output accumulated since the last take.
    pub fn take(&mut self) -> String {
        if !self.connected {
            return String::new();
        }

        if let Some(captured) = Console::get_mut().captured_output.as_mut() {
            return std::mem::take(captured);
        }

        String::new()
    }
}

impl Drop for ConsoleOutputCapture {
    fn drop(&mut self) {
        if !self.connected {
            return;
        }

        let console = Console::get_mut();

        // Log any remaining output that wasn't taken
        if let Some(output) = console.captured_output.take() {
            if !output.trim().is_empty() {
                log::info!("[Captured idle output]\n{}", output.trim_end());
            }
        }

        // Restore previous capture state
        console.captured_output = self.previous_output.take();
    }
}

/// Pending action to perform at the start of the next `r_read_console()` call.
/// This is used to implement multi-step operations that require returning
/// control to R between steps.
#[derive(Default)]
enum ReadConsolePendingAction {
    /// No pending action, proceed with normal read-console logic.
    #[default]
    None,

    /// We just evaluated `.ark_capture_top_level_environment()` to capture the
    /// top-level environment into `.ark_top_level_env`. Now retrieve it and
    /// push onto the frame stack.
    CaptureEnv,

    /// Execute a saved input upon re-entering `r_read_console()`.
    ExecuteInput(String),
}

impl Console {
    /// Sets up the main R thread, initializes the `CONSOLE` singleton,
    /// and starts R. Does not return!
    /// SAFETY: Must be called only once. Enforced with a panic.
    pub(crate) fn start(
        r_args: Vec<String>,
        startup_file: Option<String>,
        comm_event_tx: Sender<CommEvent>,
        r_request_rx: Receiver<RRequest>,
        stdin_request_tx: Sender<StdInRequest>,
        stdin_reply_rx: Receiver<amalthea::Result<InputReply>>,
        iopub_tx: Sender<IOPubMessage>,
        kernel_init_tx: Bus<KernelInfo>,
        kernel_request_rx: Receiver<KernelRequest>,
        dap: Arc<Mutex<Dap>>,
        session_mode: SessionMode,
        default_repos: DefaultRepos,
        graphics_device_rx: AsyncUnboundedReceiver<GraphicsDeviceNotification>,
        console_notification_rx: AsyncUnboundedReceiver<ConsoleNotification>,
    ) {
        // Set the main thread ID.
        // Must happen before doing anything that checks `Console::on_main_thread()`,
        // like running an `r_task()` (posit-dev/positron#4973).
        unsafe {
            CONSOLE_THREAD_ID = match CONSOLE_THREAD_ID {
                None => Some(std::thread::current().id()),
                Some(id) => panic!("`start()` must be called exactly 1 time. It has already been called from thread {id:?}."),
            };
        }

        let (tasks_interrupt_rx, tasks_idle_rx, tasks_idle_any_rx) = r_task::take_receivers();

        CONSOLE.set(UnsafeCell::new(Console::new(
            tasks_interrupt_rx,
            tasks_idle_rx,
            tasks_idle_any_rx,
            comm_event_tx,
            r_request_rx,
            stdin_request_tx,
            stdin_reply_rx,
            iopub_tx,
            kernel_request_rx,
            dap,
            session_mode,
        )));

        let console = Console::get_mut();

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

        let r_home = match r_home_setup() {
            Ok(r_home) => r_home,
            Err(err) => panic!("Can't set up `R_HOME`: {err}"),
        };

        // `R_HOME` is now defined no matter what and will be used by
        // `r_command()`. Let's discover the other important environment
        // variables set by R's shell script frontend.
        // https://github.com/posit-dev/positron/issues/3637
        match r_command(|command| {
            // From https://github.com/rstudio/rstudio/blob/74696236/src/cpp/core/r_util/REnvironmentPosix.cpp#L506-L515
            command
                .arg("--vanilla")
                .arg("-s")
                .arg("-e")
                .arg(r#"cat(paste(R.home('share'), R.home('include'), R.home('doc'), sep=';'))"#);
        }) {
            Ok(output) => {
                if let Ok(vars) = String::from_utf8(output.stdout) {
                    let vars: Vec<&str> = vars.trim().split(';').collect();
                    if vars.len() == 3 {
                        // Set the R env vars as the R shell script frontend would
                        unsafe {
                            std::env::set_var("R_SHARE_DIR", vars[0]);
                            std::env::set_var("R_INCLUDE_DIR", vars[1]);
                            std::env::set_var("R_DOC_DIR", vars[2]);
                        };
                    } else {
                        log::warn!("Unexpected output for R envvars");
                    }
                } else {
                    log::warn!("Could not read stdout for R envvars");
                };
            },
            Err(err) => log::error!("Failed to discover R envvars: {err}"),
        };

        let libraries = RLibraries::from_r_home_path(&r_home);
        libraries.initialize_pre_setup_r();

        crate::sys::console::setup_r(&r_args);

        libraries.initialize_post_setup_r();

        unsafe {
            // Register embedded routines
            r_register_routines();

            // Initialize harp (after routine registration)
            harp::initialize();

            // Optionally run a frontend specified R startup script (after harp init)
            if let Some(file) = &startup_file {
                harp::source(file)
                    .context(format!("Failed to source startup file '{file}' due to"))
                    .log_err();
            }

            // Initialize support functions (after routine registration, after
            // r_task initialization). Intentionally panic if module loading
            // fails. Modules are critical for ark to function.
            match modules::initialize() {
                Err(err) => {
                    panic!("Failed to load R modules: {err:?}");
                },
                Ok(namespace) => {
                    console.positron_ns = Some(namespace);
                },
            }

            // Populate srcrefs for namespaces already loaded in the session.
            // Namespaces of future loaded packages will be populated on load.
            // (after r_task initialization)
            if do_resource_namespaces() {
                if let Err(err) = resource_loaded_namespaces() {
                    log::error!("Can't populate srcrefs for loaded packages: {err:?}");
                }
            }

            // Set default repositories
            if let Err(err) = apply_default_repos(default_repos) {
                log::error!("Error setting default repositories: {err:?}");
            }

            // Initialise Ark's last value
            libr::SETCDR(r_symbol!(".ark_last_value"), harp::r_null());
        }

        // Now that R has started (emitting any startup messages that we capture in the
        // banner), and now that we have set up all hooks and handlers, officially finish
        // the R initialization process.
        log::info!(
            "R has started and ark handlers have been registered, completing initialization."
        );
        Self::complete_initialization(console.banner.take(), kernel_init_tx);

        // Spawn handler loop for async messages from other components (e.g., LSP).
        // Note that we do it after init is complete to avoid deadlocking
        // integration tests by spawning an async task. The deadlock is caused
        // by the `block_on()` behaviour in
        // https://github.com/posit-dev/ark/blob/bd827e73/crates/ark/src/r_task.rs#L261.
        r_task::spawn_interrupt({
            let dap_clone = console.debug_dap.clone();
            || async move {
                Console::process_console_notifications(console_notification_rx, dap_clone).await
            }
        });

        // Initialize the GD context on this thread.
        // Note that we do it after init is complete to avoid deadlocking
        // integration tests by spawning an async task. The deadlock is caused
        // by https://github.com/posit-dev/ark/blob/bd827e735970ca17102aeddfbe2c3ccf26950a36/crates/ark/src/r_task.rs#L261.
        // We should be able to remove this escape hatch in `r_task()` by
        // instantiating an `Console` in unit tests as well.
        graphics_device::init_graphics_device(
            console.get_comm_event_tx().clone(),
            console.get_iopub_tx().clone(),
            graphics_device_rx,
        );

        // Now that R has started and libr and ark have fully initialized, run site and user
        // level R profiles, in that order
        if !ignore_site_r_profile {
            startup::source_site_r_profile(&r_home);
        }
        if !ignore_user_r_profile {
            startup::source_user_r_profile();
        }

        // Start the REPL. Does not return!
        crate::sys::console::run_r();
    }

    /// Build the argument list from the command line arguments. The default
    /// list is `--interactive` unless altered with the `--` passthrough
    /// argument.
    ///
    /// # Safety
    ///
    /// The R initialization routines `Rf_initialize_R()`, `cmdlineoptions()`, and
    /// `R_common_command_line()` all modify the underlying array of C strings directly,
    /// invalidating our pointers, so we can't actually free these by reclaiming them with
    /// [CString::from_raw()]. It should be a very small memory leak though.
    pub fn build_ark_c_args(args: &Vec<String>) -> Vec<*mut c_char> {
        let mut out = Vec::with_capacity(args.len() + 1);

        cfg_if::cfg_if! {
            if #[cfg(unix)] {
                out.push(CString::new("ark").unwrap().into_raw());
            } else if #[cfg(windows)] {
                out.push(CString::new("ark.exe").unwrap().into_raw());
            } else {
                unreachable!("Unsupported OS");
            }
        }

        for arg in args {
            out.push(CString::new(arg.as_str()).unwrap().into_raw());
        }

        out
    }

    /// Completes the kernel's initialization.
    ///
    /// This is a very important part of the startup procedure for timing reasons.
    ///
    /// - It broadcasts [KernelInfo] over `kernel_init_tx`, which has two side effects:
    ///
    ///   - It unblocks a kernel-info request on shell, freeing the client to begin
    ///     sending messages of their own. We need R to be fully started up before we
    ///     can field any requests.
    ///
    ///   - It unblocks the LSP startup procedure, allowing it to start. We again need
    ///     R to be fully started up before we can initialize the LSP and field requests.
    ///
    /// # Safety
    ///
    /// Can only be called from the R thread, and only once.
    pub fn complete_initialization(banner: Option<String>, mut kernel_init_tx: Bus<KernelInfo>) {
        let version = unsafe {
            let version = Rf_findVarInFrame(R_BaseNamespace, r_symbol!("R.version.string"));
            RObject::new(version).to::<String>().unwrap()
        };

        // Initial input and continuation prompts
        let input_prompt: String = harp::get_option("prompt").try_into().unwrap();
        let continuation_prompt: String = harp::get_option("continue").try_into().unwrap();

        let kernel_info = KernelInfo {
            version: version.clone(),
            banner: banner.unwrap_or_default(),
            input_prompt: Some(input_prompt),
            continuation_prompt: Some(continuation_prompt),
        };

        log::info!("Sending kernel info: {version}");
        kernel_init_tx.broadcast(kernel_info);

        // Thread-safe initialisation flag for R
        R_INIT.set(()).expect("`R_INIT` can only be set once");
    }

    pub fn new(
        tasks_interrupt_rx: Receiver<RTask>,
        tasks_idle_rx: Receiver<RTask>,
        tasks_idle_any_rx: Receiver<RTask>,
        comm_event_tx: Sender<CommEvent>,
        r_request_rx: Receiver<RRequest>,
        stdin_request_tx: Sender<StdInRequest>,
        stdin_reply_rx: Receiver<amalthea::Result<InputReply>>,
        iopub_tx: Sender<IOPubMessage>,
        kernel_request_rx: Receiver<KernelRequest>,
        dap: Arc<Mutex<Dap>>,
        session_mode: SessionMode,
    ) -> Self {
        Self {
            r_request_rx,
            comm_event_tx,
            stdin_request_tx,
            stdin_reply_rx,
            iopub_tx,
            kernel_request_rx,
            active_request: None,
            execution_count: 0,
            autoprint_output: String::new(),
            ui_comm_tx: None,
            last_error: None,
            help_event_tx: None,
            help_port: None,
            lsp_events_tx: None,
            lsp_virtual_documents: HashMap::new(),
            debug_dap: dap,
            debug_is_debugging: false,
            debug_stopped_reason: None,
            tasks_interrupt_rx,
            tasks_idle_rx,
            tasks_idle_any_rx,
            pending_futures: HashMap::new(),
            session_mode,
            positron_ns: None,
            banner: None,
            r_error_buffer: None,
            captured_output: None,
            debug_call_text: DebugCallText::None,
            debug_last_line: None,
            debug_transient_eval: false,
            debug_last_stack: vec![],
            debug_session_index: 1,
            debug_current_frame_id: 0,
            debug_selected_frame_id: Cell::new(None),
            debug_jit_level: None,
            pending_inputs: None,
            read_console_depth: Cell::new(0),
            read_console_nested_return: Cell::new(false),
            read_console_threw_error: Cell::new(false),
            read_console_pending_action: Cell::new(ReadConsolePendingAction::None),
            read_console_env_stack: RefCell::new(Vec::new()),
            read_console_shutdown: Cell::new(false),
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

    /// Has the `Console` singleton completed initialization.
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
    /// SAFETY: Accesses must occur after `Console::start()` initializes it.
    pub fn get() -> &'static Self {
        Console::get_mut()
    }

    /// Access a mutable reference to the singleton instance of this struct
    ///
    /// SAFETY: Accesses must occur after `Console::start()` initializes it.
    /// Be aware that we're bypassing the borrow checker. The only guarantee we
    /// have is that `CONSOLE` is only accessed from the R thread. If you're
    /// inspecting mutable state, or mutating state, you must reason the
    /// soundness by yourself.
    pub fn get_mut() -> &'static mut Self {
        CONSOLE.with_borrow_mut(|cell| {
            let console_ref = cell.get_mut();

            // We extend the lifetime to `'static` as `CONSOLE` is effectively static once initialized.
            // This allows us to return a `&mut` from the unsafe cell to the caller.
            unsafe { std::mem::transmute::<&mut Console, &'static mut Console>(console_ref) }
        })
    }

    pub fn on_main_thread() -> bool {
        let thread = std::thread::current();
        thread.id() == unsafe { CONSOLE_THREAD_ID.unwrap() }
    }

    /// Provides read-only access to `iopub_tx`
    pub fn get_iopub_tx(&self) -> &Sender<IOPubMessage> {
        &self.iopub_tx
    }

    /// Start capturing console output.
    /// Returns a guard that saves and restores the previous capture state on drop.
    pub(crate) fn start_capture(&mut self) -> ConsoleOutputCapture {
        let previous_output = self.captured_output.replace(String::new());

        ConsoleOutputCapture {
            previous_output,
            connected: true,
        }
    }

    /// Get the current execution context if an active request exists.
    /// Returns (execution_id, code) tuple where execution_id is the Jupyter message ID.
    pub fn get_execution_context(&self) -> Option<(String, String)> {
        self.active_request.as_ref().map(|req| {
            (
                req.originator.header.msg_id.clone(),
                req.request.code.clone(),
            )
        })
    }

    // Async messages for the Console. Processed at interrupt time.
    async fn process_console_notifications(
        mut console_notification_rx: AsyncUnboundedReceiver<ConsoleNotification>,
        dap: Arc<Mutex<Dap>>,
    ) {
        loop {
            while let Some(notification) = console_notification_rx.recv().await {
                match notification {
                    ConsoleNotification::DidChangeDocument(uri) => {
                        let mut dap = dap.lock().unwrap();
                        dap.did_change_document(&uri);
                    },
                }
            }
        }
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

        let loc = req.code_location().log_err().flatten().map(|mut loc| {
            // Normalize URI for Windows compatibility. Positron sends URIs like
            // `file:///c%3A/...` which do not match DAP's breakpoint path keys.
            loc.uri = ExtUrl::normalize(loc.uri);
            loc
        });

        // Return the code to the R console to be evaluated and the corresponding exec count
        (
            ConsoleInput::Input(req.code.clone(), loc),
            self.execution_count,
        )
    }

    /// Invoked by R to read console input from the user.
    ///
    /// * `prompt` - The prompt shown to the user
    /// * `buf`    - Pointer to buffer to receive the user's input (type `CONSOLE_BUFFER_CHAR`)
    /// * `buflen` - Size of the buffer to receiver user's input
    /// * `_hist`   - Whether to add the input to the history (1) or not (0)
    ///
    /// This does two things:
    /// - Move the Console state machine to the next state:
    ///   - Wait for input
    ///   - Set an active execute request and a list of pending expressions
    ///   - Set `self.debug_is_debugging` depending on presence or absence of debugger prompt
    ///   - Evaluate next pending expression
    ///   - Close active execute request if pending list is empty
    /// - Run an event loop while waiting for input
    fn read_console(
        &mut self,
        prompt: *const c_char,
        buf: *mut c_uchar,
        buflen: c_int,
        _hist: c_int,
    ) -> ConsoleResult {
        self.debug_handle_read_console();

        // State machine part of ReadConsole

        let info = self.prompt_info(prompt);
        log::trace!("R prompt: {}", info.input_prompt);

        // Invariant: If we detect a browser prompt, `self.debug_is_debugging`
        // is true. Otherwise it is false.
        if matches!(info.kind, PromptKind::Browser) {
            // Check for auto-stepping first. If we're going to auto-step, don't
            // emit start_debug/stop_debug messages and don't close active
            // request. These intermediate steps are still part of the ongoing
            // request.
            if let Some(result) = self.maybe_auto_step(buf, buflen) {
                return result;
            }

            // Similarly, if we have pending inputs, we're about to immediately
            // continue with the next expression. Don't emit debug notifications
            // for these intermediate browser prompts.
            let has_pending = self.pending_inputs.as_ref().is_some_and(|p| !p.is_empty());

            // Only now that we know we're stopping for real, set state and
            // notify frontend. Note that for simplicity this state is reset on
            // exit via the cleanups registered in `r_read_console()`. Ideally
            // we'd clean from here for symmetry.
            self.debug_is_debugging = true;
            if !has_pending {
                let reason = self
                    .debug_stopped_reason
                    .clone()
                    .unwrap_or(DebugStoppedReason::Step);
                self.debug_start(self.debug_transient_eval, reason);
            }
        }

        if let Some(exception) = self.take_exception() {
            // We might get an input request if `readline()` or `menu()` is
            // called in `options(error = )`. We respond to this with an error
            // as this is not supported by Ark.
            if matches!(info.kind, PromptKind::InputRequest) {
                // Reset error so we can handle it when we recurse here after
                // the error aborts the readline. Note it's better to first emit
                // the R invalid input request error, and then handle
                // `exception` within the context of a new `ReadConsole`
                // instance, so that we emit the proper execution prompts as
                // part of the response, and not the readline prompt.
                self.last_error = Some(exception);
                return self.handle_invalid_input_request_after_error();
            }

            // Clear any pending inputs, if any
            self.pending_inputs = None;

            // Reply to active request with error, then fall through to event loop
            self.handle_active_request(&info, ConsoleValue::Error(exception));
        } else if matches!(info.kind, PromptKind::InputRequest) {
            // Request input from the frontend and return it to R
            return self.handle_input_request(&info, buf, buflen);
        } else if let Some(input) = self.pop_pending() {
            // Evaluate pending expression if there is any remaining
            return self.handle_pending_input(input, buf, buflen);
        } else {
            // Otherwise reply to active request with accumulated result, then
            // fall through to event loop
            let result = self.take_result();
            self.handle_active_request(&info, ConsoleValue::Success(result));
        }

        // In the future we'll also send browser information, see
        // https://github.com/posit-dev/positron/issues/3001. Currently this is
        // a push model where we send the console inputs at each round. In the
        // future, a pull model would be better, this way the LSP can manage a
        // cache of inputs and we don't need to retraverse the environments as
        // often. We'd still push a `DidChangeConsoleInputs` notification from
        // here, but only containing high-level information such as `search()`
        // contents and `ls(rho)`.
        if !self.debug_is_debugging && !matches!(info.kind, PromptKind::InputRequest) {
            self.refresh_lsp();
        }

        // Signal prompt
        EVENTS.console_prompt.emit(());

        self.run_event_loop(&info, buf, buflen, WaitFor::ExecuteRequest)
    }

    /// Runs the ReadConsole event loop.
    /// This handles events for:
    /// - Reception of either input replies or execute requests (as determined
    ///   by `wait_for`)
    /// - Idle-time and interrupt-time tasks
    /// - Requests from the frontend (currently only used for establishing UI comm)
    /// - R's polled events
    fn run_event_loop(
        &mut self,
        info: &PromptInfo,
        buf: *mut c_uchar,
        buflen: c_int,
        wait_for: WaitFor,
    ) -> ConsoleResult {
        let mut select = crossbeam::channel::Select::new();

        // Cloning is necessary to avoid a double mutable borrow error
        let r_request_rx = self.r_request_rx.clone();
        let stdin_reply_rx = self.stdin_reply_rx.clone();
        let kernel_request_rx = self.kernel_request_rx.clone();
        let tasks_interrupt_rx = self.tasks_interrupt_rx.clone();
        let tasks_idle_rx = self.tasks_idle_rx.clone();
        let tasks_idle_any_rx = self.tasks_idle_any_rx.clone();

        // Process R's polled events regularly while waiting for console input.
        // We used to poll every 200ms but that lead to visible delays for the
        // processing of plot events, it also slowed down callbacks from the later
        // package. 50ms seems to be more in line with RStudio (posit-dev/positron#7235).
        let polled_events_rx = crossbeam::channel::tick(Duration::from_millis(50));

        // This is the main kind of message from the frontend that we are expecting.
        // We either wait for `input_reply` messages on StdIn, or for
        // `execute_request` on Shell.
        let (r_request_index, stdin_reply_index) = match wait_for {
            WaitFor::ExecuteRequest => (Some(select.recv(&r_request_rx)), None),
            WaitFor::InputReply => (None, Some(select.recv(&stdin_reply_rx))),
        };

        let kernel_request_index = select.recv(&kernel_request_rx);
        let tasks_interrupt_index = select.recv(&tasks_interrupt_rx);
        let polled_events_index = select.recv(&polled_events_rx);

        // Only process idle at top level. We currently don't want idle tasks
        // (e.g. for srcref generation) to run when the call stack is not empty.
        let tasks_idle_index = if matches!(info.kind, PromptKind::TopLevel) {
            Some(select.recv(&tasks_idle_rx))
        } else {
            None
        };

        // "Idle any" tasks run at both top-level and browser prompts
        let tasks_idle_any_index =
            if matches!(info.kind, PromptKind::TopLevel | PromptKind::Browser) {
                Some(select.recv(&tasks_idle_any_rx))
            } else {
                None
            };

        loop {
            // If an interrupt was signaled and we are in a user
            // request prompt, e.g. `readline()`, we need to propagate
            // the interrupt to the R stack. This needs to happen before
            // `process_idle_events()`, particularly on Windows, because it
            // calls `R_ProcessEvents()`, which checks and resets
            // `UserBreak`, but won't actually fire the interrupt b/c
            // we have them disabled, so it would end up swallowing the
            // user interrupt request.
            if matches!(info.kind, PromptKind::InputRequest) && interrupts_pending() {
                return ConsoleResult::Interrupt;
            }

            // Otherwise we are at top level and we can assume the
            // interrupt was 'handled' on the frontend side and so
            // reset the flag
            set_interrupts_pending(false);

            // First handle execute requests outside of `select` to ensure they
            // have priority. `select` chooses at random.
            if let WaitFor::ExecuteRequest = wait_for {
                if let Ok(req) = r_request_rx.try_recv() {
                    if let Some(input) = self.handle_execute_request(req, &info, buf, buflen) {
                        return input;
                    }
                }
            }

            let oper = select.select();

            match oper.index() {
                // We've got an execute request from the frontend
                i if Some(i) == r_request_index => {
                    let req = oper.recv(&r_request_rx);
                    let Ok(req) = req else {
                        // The channel is disconnected and empty
                        return ConsoleResult::Disconnected;
                    };

                    if let Some(input) = self.handle_execute_request(req, &info, buf, buflen) {
                        return input;
                    }
                },

                // We've got a reply for readline
                i if Some(i) == stdin_reply_index => {
                    let reply = oper.recv(&stdin_reply_rx).unwrap();
                    return self.handle_input_reply(reply, buf, buflen);
                },

                // We've got a kernel request
                i if i == kernel_request_index => {
                    let req = oper.recv(&kernel_request_rx).unwrap();
                    self.handle_kernel_request(req, &info);
                },

                // An interrupt task woke us up
                i if i == tasks_interrupt_index => {
                    let task = oper.recv(&tasks_interrupt_rx).unwrap();
                    self.handle_task_interrupt(task);
                },

                // An idle task woke us up
                i if Some(i) == tasks_idle_index => {
                    let task = oper.recv(&tasks_idle_rx).unwrap();
                    self.handle_task(task);
                },

                // An "idle any" task woke us up
                i if Some(i) == tasks_idle_any_index => {
                    let task = oper.recv(&tasks_idle_any_rx).unwrap();
                    self.handle_task(task);
                },

                // It's time to run R's polled events
                i if i == polled_events_index => {
                    let _ = oper.recv(&polled_events_rx).unwrap();
                    Self::process_idle_events();
                },

                i => log::error!("Unexpected index in Select: {i}"),
            }
        }
    }

    // We prefer to panic if there is an error while trying to determine the
    // prompt type because any confusion here is prone to put the frontend in a
    // bad state (e.g. causing freezes)
    fn prompt_info(&self, prompt_c: *const c_char) -> PromptInfo {
        let n_frame = harp::session::r_n_frame().unwrap();
        log::trace!("prompt_info(): n_frame = '{n_frame}'");

        let prompt_slice = unsafe { CStr::from_ptr(prompt_c) };
        let prompt = prompt_slice.to_string_lossy().into_owned();

        // Sent to the frontend after each top-level command so users can
        // customise their prompts
        let continuation_prompt: String = harp::get_option("continue").try_into().unwrap();

        // Detect browser prompt by matching the prompt string
        // https://github.com/posit-dev/positron/issues/4742.
        // There are ways to break this detection, for instance setting
        // `options(prompt =, continue = ` to something that looks like
        // a browser prompt, or doing the same with `readline()`. We have
        // chosen to not support these edge cases.
        let browser = RE_DEBUG_PROMPT.is_match(&prompt);

        // Determine the prompt kind based on context
        let kind = if browser {
            PromptKind::Browser
        } else if n_frame > 0 {
            // If there are frames on the stack and we're not in a browser prompt,
            // this means some user code is requesting input, e.g. via `readline()`
            PromptKind::InputRequest
        } else {
            PromptKind::TopLevel
        };

        return PromptInfo {
            input_prompt: prompt,
            continuation_prompt,
            kind,
        };
    }

    /// Take result from `self.autoprint_output` and R's `.Last.value` object
    fn take_result(&mut self) -> serde_json::Map<String, serde_json::Value> {
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
            // Jupyter frontends are not expecting
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
                    Ok(html) => {
                        data.insert("text/html".to_string(), json!(html));
                    },
                    Err(err) => {
                        log::error!("{:?}", err);
                    },
                };
            }
        }

        data
    }

    /// Reset debug flag on the global environment.
    ///
    /// This is a workaround for when a breakpoint was entered at top-level, in
    /// a `{}` block. In that case `browser()` marks the global environment as
    /// being debugged here:
    /// https://github.com/r-devel/r-svn/blob/476ffd4c/src/main/main.c#L1492-L1494.
    ///
    /// Only do it when the call stack is empty, as removing the flag prevents
    /// normal stepping with `source()`.
    fn reset_global_env_rdebug(&self) {
        if harp::r_n_frame().unwrap_or(0) == 0 {
            unsafe { libr::SET_RDEBUG(libr::R_GlobalEnv, 0) };
        }
    }

    fn take_exception(&mut self) -> Option<Exception> {
        let mut exception = if let Some(exception) = self.last_error.take() {
            exception
        } else if stack_overflow_occurred() {
            // Call `base::traceback()` since we don't have a handled error
            // object carrying a backtrace. This won't be formatted as a
            // tree which is just as well since the recursive calls would
            // push a tree too far to the right.
            let traceback = r_traceback();

            let exception = Exception {
                ename: String::from(""),
                evalue: r_peek_error_buffer(),
                traceback,
            };

            // Reset error buffer so we don't display this message again
            let _ = RFunction::new("base", "stop").call();

            exception
        } else {
            return None;
        };

        // Flush any accumulated output to StdOut. This can happen if
        // the last input errors out during autoprint.
        let autoprint = std::mem::take(&mut self.autoprint_output);
        if !autoprint.is_empty() {
            let message = IOPubMessage::Stream(StreamOutput {
                name: Stream::Stdout,
                text: autoprint,
            });
            self.iopub_tx.send(message).unwrap();
        }

        // Jupyter clients typically discard the `evalue` when a `traceback` is
        // present.  Jupyter-Console even disregards `evalue` in all cases. So
        // include it here if we are in Notebook mode. But should Positron
        // implement similar behaviour as the other frontends eventually? The
        // first component of `traceback` could be compared to `evalue` and
        // discarded from the traceback if the same.
        if let SessionMode::Notebook = self.session_mode {
            exception.traceback.insert(0, exception.evalue.clone());
        }

        Some(exception)
    }

    fn handle_active_request(&mut self, info: &PromptInfo, value: ConsoleValue) {
        self.reset_global_env_rdebug();

        // If we get here we finished evaluating all pending inputs. Check if we
        // have an active request from a previous `read_console()` iteration. If
        // so, we `take()` and clear the `active_request` as we're about to
        // complete it and send a reply to unblock the active Shell request.
        if let Some(req) = std::mem::take(&mut self.active_request) {
            // Perform a refresh of the frontend state (Prompts, working
            // directory, etc)
            self.with_mut_ui_comm_tx(|ui_comm_tx| {
                let input_prompt = info.input_prompt.clone();
                let continuation_prompt = info.continuation_prompt.clone();

                ui_comm_tx.send_refresh(input_prompt, continuation_prompt);
            });

            // Check for pending graphics updates
            // (Important that this occurs while in the "busy" state of this ExecuteRequest
            // so that the `parent` message is set correctly in any Jupyter messages)
            graphics_device::on_did_execute_request();

            // Let frontend know the last request is complete. This turns us
            // back to Idle.
            Self::reply_execute_request(&self.iopub_tx, req, value);
        } else {
            log::info!("No active request to handle, discarding: {value:?}");
        }
    }

    // Called from Ark's ReadConsole event loop when we get a new execute
    // request. It's not possible to get one while an active request is ongoing
    // because of Jupyter's queueing of Shell messages.
    fn handle_execute_request(
        &mut self,
        req: RRequest,
        info: &PromptInfo,
        buf: *mut c_uchar,
        buflen: c_int,
    ) -> Option<ConsoleResult> {
        if matches!(info.kind, PromptKind::InputRequest) {
            panic!("Unexpected `execute_request` while waiting for `input_reply`.");
        }

        let input = match req {
            RRequest::ExecuteCode(exec_req, originator, reply_tx) => {
                // Extract input from request
                let (input, exec_count) = { self.init_execute_request(&exec_req) };

                // Save `ExecuteCode` request so we can respond to it at next prompt
                self.active_request = Some(ActiveReadConsoleRequest {
                    exec_count,
                    request: exec_req.clone(),
                    originator: originator.clone(),
                    reply_tx,
                });

                // Push execution context to graphics device for plot attribution
                graphics_device::on_execute_request(
                    originator.header.msg_id.clone(),
                    exec_req.code.clone(),
                );

                input
            },

            RRequest::Shutdown(_) => ConsoleInput::EOF,

            RRequest::DebugCommand(cmd) => {
                // Just ignore command in case we left the debugging state already
                if !self.debug_is_debugging {
                    return None;
                }

                // Translate requests from the debugger frontend to actual inputs for
                // the debug interpreter
                ConsoleInput::Input(debug_request_command(cmd), None)
            },
        };

        match input {
            ConsoleInput::Input(code, loc) => {
                // Parse input into pending expressions

                // Keep the DAP lock while we are updating breakpoints
                let mut dap_guard = self.debug_dap.lock().unwrap();
                let uri = loc.as_ref().map(|l| l.uri.clone());
                let breakpoints = uri
                    .as_ref()
                    .and_then(|uri| dap_guard.breakpoints.get_mut(uri))
                    .map(|(_, v)| v.as_mut_slice());

                match PendingInputs::read(&code, loc, breakpoints) {
                    Ok(ParseResult::Success(inputs)) => {
                        self.pending_inputs = inputs;
                    },
                    Ok(ParseResult::SyntaxError(message)) => {
                        return Some(ConsoleResult::Error(message));
                    },
                    Err(err) => {
                        return Some(ConsoleResult::Error(format!(
                            "Error while parsing input: {err:?}"
                        )));
                    },
                }

                // Notify frontend about any breakpoints marked invalid during annotation.
                // Remove disabled breakpoints.
                if let Some(uri) = &uri {
                    dap_guard.notify_invalid_breakpoints(uri);
                    dap_guard.remove_disabled_breakpoints(uri);
                }

                drop(dap_guard);

                // Evaluate first expression if there is one
                if let Some(input) = self.pop_pending() {
                    Some(self.handle_pending_input(input, buf, buflen))
                } else {
                    if self.debug_is_debugging &&
                        !harp::options::get_option_bool("browserNLdisabled")
                    {
                        // Empty input in the debugger counts as `n` unless
                        // `browserNLdisabled` is TRUE. This matches RStudio
                        // and base R behaviour.
                        // https://github.com/posit-dev/ark/issues/1006
                        Some(self.debug_forward_command(buf, buflen, String::from("n")))
                    } else {
                        // Otherwise we got an empty input, e.g. `""` and there's
                        // nothing to do. Close active request.
                        self.handle_active_request(info, ConsoleValue::Success(Default::default()));

                        // And return to event loop
                        None
                    }
                }
            },

            ConsoleInput::EOF => Some(ConsoleResult::Disconnected),
        }
    }

    /// Handles user input requests (e.g., readline, menu) and special prompts.
    /// Runs the ReadConsole event loop until a reply comes in.
    fn handle_input_request(
        &mut self,
        info: &PromptInfo,
        buf: *mut c_uchar,
        buflen: c_int,
    ) -> ConsoleResult {
        if let Some(req) = &self.active_request {
            // Send request to frontend. We'll wait for an `input_reply`
            // from the frontend in the event loop in `read_console()`.
            // The active request remains active.
            self.request_input(req.originator.clone(), String::from(&info.input_prompt));

            // Run the event loop, waiting for stdin replies but not execute requests
            self.run_event_loop(info, buf, buflen, WaitFor::InputReply)
        } else {
            // Invalid input request, propagate error to R
            self.handle_invalid_input_request(buf, buflen)
        }
    }

    fn handle_pending_input(
        &mut self,
        input: PendingInput,
        buf: *mut c_uchar,
        buflen: c_int,
    ) -> ConsoleResult {
        // Default: Mark evaluation as transient.
        // This only has an effect if we're debugging.
        // https://github.com/posit-dev/positron/issues/3151
        self.debug_transient_eval = true;

        if self.debug_is_debugging {
            // Try to interpret this pending input as a symbol (debug commands
            // are entered as symbols).
            if let Ok(sym) = harp::RSymbol::new(input.expr.sexp) {
                let mut sym = String::from(sym);

                // When stopped at an exception breakpoint or pause, the top
                // frame is the hidden handler that called `browser()`. Remap
                // "step over" to "step out" so the user leaves the handler
                // frame instead of stepping through internal code.
                if sym == "n" &&
                    matches!(
                        self.debug_stopped_reason,
                        Some(DebugStoppedReason::Condition { .. } | DebugStoppedReason::Pause)
                    )
                {
                    sym = String::from("f");
                }

                if DEBUG_COMMANDS.contains(&&sym[..]) {
                    return self.debug_forward_command(buf, buflen, sym);
                }
            }
        }

        ConsoleResult::NewPendingInput(input)
    }

    /// Forward a debug command to R's base REPL.
    fn debug_forward_command(
        &mut self,
        buf: *mut c_uchar,
        buflen: c_int,
        cmd: String,
    ) -> ConsoleResult {
        debug_assert!(
            DEBUG_COMMANDS.contains(&&cmd[..]),
            "Expected a debug command, got: {cmd}"
        );

        if cmd == "s" {
            // Disable JIT before stepping in to prevent the confusing
            // experience of stepping into `compiler` internals.
            // https://github.com/posit-dev/positron/issues/11890
            match harp::parse_eval_base("compiler::enableJIT(0L)").and_then(i32::try_from) {
                Ok(old) => self.debug_jit_level = Some(old),
                Err(err) => log::error!("Failed to disable JIT: {err:?}"),
            }
        }

        if DEBUG_COMMANDS_CONTINUE.contains(&&cmd[..]) {
            // Navigation commands are not transient evals.
            self.debug_transient_eval = false;
        }

        // Forward the command to R's base REPL.
        // Unwrap safety: A debug command fits in the buffer.
        Self::on_console_input(buf, buflen, cmd).unwrap();
        ConsoleResult::NewInput
    }

    fn pop_pending(&mut self) -> Option<PendingInput> {
        let Some(pending_inputs) = self.pending_inputs.as_mut() else {
            return None;
        };

        let Some(input) = pending_inputs.pop() else {
            self.pending_inputs = None;
            return None;
        };

        if pending_inputs.is_empty() {
            self.pending_inputs = None;
        }

        Some(input)
    }

    // SAFETY: Call this from a POD frame. Inputs must be protected.
    unsafe fn eval(
        &self,
        expr: libr::SEXP,
        srcref: libr::SEXP,
        buf: *mut c_uchar,
        buflen: c_int,
        is_debugging: bool,
    ) {
        // SAFETY: This may jump in case of error, keep this POD
        unsafe {
            let frame = libr::Rf_protect(self.eval_frame().sexp);

            // The global source reference is stored in this global variable by
            // the R REPL before evaluation. We do the same here.
            let old_srcref = libr::Rf_protect(libr::get(libr::R_Srcref));
            libr::set(libr::R_Srcref, srcref);

            // Beware: this may throw an R longjump.
            let value = if is_debugging {
                // When debugging, install our error handler as a local calling
                // handler so it fires before R's own error handler. This ensures
                // proper backtrace capturing and, when error exception breakpoints
                // are enabled, correct stopped reason for the DAP event. We could
                // eventually set our error handler in this way for all evals, but
                // for now make it conditional on debugging as this is a new
                // approach.
                let mut body_data = EvalBodyData { expr, frame };
                with_calling_error_handler(
                    eval_body_callback,
                    &mut body_data as *mut _ as *mut c_void,
                    eval_error_callback,
                    std::ptr::null_mut(),
                )
            } else {
                libr::Rf_eval(expr, frame)
            };
            libr::Rf_protect(value);

            // Restore `R_Srcref`, necessary at least to avoid messing with
            // DAP's last frame info
            libr::set(libr::R_Srcref, old_srcref);

            // Store in the base environment for robust access from (almost) any
            // evaluation environment. We only require the presence of `::` so
            // we can reach into base. Note that unlike regular environments
            // which are stored in pairlists or hash tables, the base environment
            // is stored in the `value` field of symbols, i.e. their "CDR".
            libr::SETCDR(r_symbol!(".ark_last_value"), value);

            libr::Rf_unprotect(3);
            value
        };

        // Back in business, Rust away
        let code = if unsafe { libr::get(libr::R_Visible) == 1 } {
            String::from("base::.ark_last_value")
        } else {
            String::from("base::invisible(base::.ark_last_value)")
        };

        // Unwrap safety: The input always fits in the buffer
        Self::on_console_input(buf, buflen, code).unwrap();
    }

    /// Resolve the frame in which to evaluate the current expression.
    /// Uses the debug-selected frame if one has been set, otherwise the current frame.
    fn eval_frame(&self) -> harp::RObject {
        let Some(frame_id) = self.debug_selected_frame_id.get() else {
            return harp::r_current_frame();
        };

        let state = self.debug_dap.lock().unwrap();
        match state.frame_env(Some(frame_id)) {
            Ok(env) => harp::RObject::view(env),
            Err(err) => {
                log::warn!("Failed to resolve selected frame {frame_id}: {err}");
                harp::r_current_frame()
            },
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
    /// We make a single exception for renv `activate.R` scripts, because it is easy for
    /// them to get outdated, and we want them to at least be able to start up:
    /// - In renv >=1.0.9, renv never calls `readline()` from within `.Rprofile` and
    ///   everything works as it should.
    /// - In renv 1.0.2 to 1.0.8, renv calls `readline()` using `renv:::ask()`, and we
    ///   return `"n"` immediately rather than letting the user respond.
    /// - In renv <=1.0.1, renv calls `readline()` using `renv:::menu()`, and we
    ///   return `"Leave project library empty"` immediately rather than letting the user
    ///   respond.
    ///
    /// https://github.com/rstudio/renv/pull/1915
    /// https://github.com/posit-dev/positron/issues/2070
    /// https://github.com/rstudio/renv/blob/5d0d52c395e569f7f24df4288d949cef95efca4e/inst/resources/activate.R#L85-L87
    fn handle_invalid_input_request(&self, buf: *mut c_uchar, buflen: c_int) -> ConsoleResult {
        if let Some(input) = Self::renv_autoloader_reply() {
            log::warn!("Detected `readline()` call in renv autoloader. Returning `'{input}'`.");
            match Self::on_console_input(buf, buflen, input) {
                Ok(()) => return ConsoleResult::NewInput,
                Err(err) => return ConsoleResult::Error(format!("{err}")),
            }
        }

        log::warn!("Detected invalid `input_request` outside an `execute_request`. Preparing to throw an R error.");

        let message = vec![
            "Can't request input from the user at this time.",
            "Are you calling `readline()` or `menu()` from an `.Rprofile` or `.Rprofile.site` file? If so, that is the issue and you should remove that code."
        ].join("\n");

        return ConsoleResult::Error(message);
    }

    fn handle_invalid_input_request_after_error(&self) -> ConsoleResult {
        log::warn!("Detected invalid `input_request` after error (probably from `getOption('error')`). Preparing to throw an R error.");

        let message = vec![
            "Can't request input from the user at this time.",
            "Are you calling `readline()` or `menu()` from `options(error = )`?",
        ]
        .join("\n");

        return ConsoleResult::Error(message);
    }

    /// Load `fallback_sources` with this stack's text sources
    /// @returns Map of `source` -> `source_reference` used for frames that don't have
    /// associated files (i.e. no `srcref` attribute). The `source` is the key to
    /// ensure that we don't insert the same function multiple times, which would result
    /// in duplicate virtual editors being opened on the client side.
    pub fn load_fallback_sources(
        &mut self,
        stack: &Vec<crate::console_debug::FrameInfo>,
    ) -> HashMap<String, String> {
        let mut sources = HashMap::new();

        for frame in stack.iter() {
            if let crate::console_debug::FrameSource::Text(source) = &frame.source {
                let uri = Self::ark_debug_uri(self.debug_session_index, &frame.source_name, source);

                if self.has_virtual_document(&uri) {
                    continue;
                }

                self.insert_virtual_document(uri.clone(), source.clone());
                sources.insert(source.clone(), uri);
            }
        }

        sources
    }

    pub fn clear_fallback_sources(&mut self) {
        // Find and close URIs associated with debug sessions. We go in two
        // steps here because we can't remove stuff from
        // `self.lsp_virtual_documents` while borrowing it to loop over it.
        let mut debug_uris = Vec::new();
        for (uri, _) in &self.lsp_virtual_documents {
            if Self::is_ark_debug_path(uri) {
                debug_uris.push(uri.clone());
            }
        }

        for uri in debug_uris {
            self.remove_virtual_document(uri);
        }
    }

    fn renv_autoloader_reply() -> Option<String> {
        let is_autoloader_running = harp::get_option("renv.autoloader.running")
            .try_into()
            .unwrap_or(false);

        if !is_autoloader_running {
            return None;
        }

        if Self::is_renv_1_0_1_or_earlier()? {
            // Response to specific `renv:::menu()` call
            Some(String::from("Leave project library empty"))
        } else {
            // Response to `renv:::ask()` call
            Some(String::from("n"))
        }
    }

    fn is_renv_1_0_1_or_earlier() -> Option<bool> {
        let result = match RFunction::from("is_renv_1_0_1_or_earlier").call_in(ARK_ENVS.positron_ns)
        {
            Ok(result) => result,
            Err(error) => {
                log::error!("Failed to call `is_renv_1_0_1_or_earlier()`: {error:?}");
                return None;
            },
        };

        let result: bool = match result.try_into() {
            Ok(result) => result,
            Err(error) => {
                log::error!(
                    "Failed to convert result of `is_renv_1_0_1_or_earlier()` to bool: {error:?}"
                );
                return None;
            },
        };

        Some(result)
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
                    Err(err) => ConsoleResult::Error(format!("{err:?}")),
                }
            },
            Err(err) => ConsoleResult::Error(format!("{err:?}")),
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

    fn handle_kernel_request(&mut self, req: KernelRequest, info: &PromptInfo) {
        log::trace!("Received kernel request {req:?}");

        match req {
            KernelRequest::EstablishUiCommChannel(ref ui_comm_tx) => {
                self.handle_establish_ui_comm_channel(ui_comm_tx.clone(), info)
            },
        };
    }

    fn handle_establish_ui_comm_channel(
        &mut self,
        ui_comm_tx: Sender<UiCommMessage>,
        info: &PromptInfo,
    ) {
        if self.ui_comm_tx.is_some() {
            log::info!("Replacing an existing UI comm channel.");
        }

        // Create and store the sender channel
        self.ui_comm_tx = Some(UiCommSender::new(ui_comm_tx));

        // Go ahead and do an initial refresh
        self.with_mut_ui_comm_tx(|ui_comm_tx| {
            let input_prompt = info.input_prompt.clone();
            let continuation_prompt = info.continuation_prompt.clone();

            ui_comm_tx.send_refresh(input_prompt, continuation_prompt);
        });
    }

    pub fn session_mode(&self) -> SessionMode {
        self.session_mode
    }

    pub fn get_ui_comm_tx(&self) -> Option<&UiCommSender> {
        self.ui_comm_tx.as_ref()
    }

    fn get_mut_ui_comm_tx(&mut self) -> Option<&mut UiCommSender> {
        self.ui_comm_tx.as_mut()
    }

    fn with_ui_comm_tx<F>(&self, f: F)
    where
        F: FnOnce(&UiCommSender),
    {
        match self.get_ui_comm_tx() {
            Some(ui_comm_tx) => f(ui_comm_tx),
            None => {
                // Trace level logging, its typically not a bug if the frontend
                // isn't connected. Happens in all Jupyter use cases.
                log::trace!("UI comm isn't connected, dropping `f`.");
            },
        }
    }

    fn with_mut_ui_comm_tx<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut UiCommSender),
    {
        match self.get_mut_ui_comm_tx() {
            Some(ui_comm_tx) => f(ui_comm_tx),
            None => {
                // Trace level logging, its typically not a bug if the frontend
                // isn't connected. Happens in all Jupyter use cases.
                log::trace!("UI comm isn't connected, dropping `f`.");
            },
        }
    }

    pub fn is_ui_comm_connected(&self) -> bool {
        self.get_ui_comm_tx().is_some()
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

    fn console_input(buf: *mut c_uchar, _buflen: c_int) -> String {
        unsafe {
            let cstr = CStr::from_ptr(buf as *const c_char);
            cstr.to_string_lossy().into_owned()
        }
    }

    // Hitting this means a SINGLE line from the user was longer than the buffer size (>4000 characters)
    fn buffer_overflow_error() -> amalthea::Error {
        Error::InvalidConsoleInput(String::from(
            "Can't pass console input on to R, a single line exceeds R's internal console buffer size."
        ))
    }

    // Reply to the previously active request. The current prompt type and
    // whether an error has occurred defines the reply kind.
    fn reply_execute_request(
        iopub_tx: &Sender<IOPubMessage>,
        req: ActiveReadConsoleRequest,
        value: ConsoleValue,
    ) {
        log::trace!("Completing execution after receiving prompt");

        let exec_count = req.exec_count;

        let (reply, result) = match value {
            ConsoleValue::Success(data) => {
                let reply = Ok(ExecuteReply {
                    status: Status::Ok,
                    execution_count: exec_count,
                    user_expressions: json!({}),
                });

                let result = if data.len() > 0 {
                    Some(IOPubMessage::ExecuteResult(ExecuteResult {
                        execution_count: exec_count,
                        data: serde_json::Value::Object(data),
                        metadata: json!({}),
                    }))
                } else {
                    None
                };

                (reply, result)
            },

            ConsoleValue::Error(exception) => {
                let reply = Err(amalthea::Error::ShellErrorExecuteReply(
                    exception.clone(),
                    exec_count,
                ));
                let result = IOPubMessage::ExecuteError(ExecuteError { exception });

                (reply, Some(result))
            },
        };

        if let Some(result) = result {
            iopub_tx.send(result).unwrap();
        }

        log::trace!("Sending `execute_reply`: {reply:?}");
        req.reply_tx.send(reply).unwrap();
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
            log::error!("Failed to receive wait reply from iopub: {error:?}");
        }
    }

    /// Request input from frontend in case code like `readline()` is
    /// waiting for input
    fn request_input(&self, originator: Originator, prompt: String) {
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
                originator,
                request: InputRequest {
                    prompt,
                    password: false,
                },
            })),
            Err(err) => panic!("Could not send input request: {}", err)
        )
    }

    /// Check if we need to auto-step through injected code.
    ///
    /// If debugger is active, to prevent injected expressions from
    /// interfering with debug-stepping, we might need to automatically step
    /// over to the next statement by returning `n` to R. Two cases:
    /// - We've just stopped due to an injected breakpoint. In this case
    ///   we're in the `.ark_breakpoint()` function and can look at the current
    ///   `sys.function()` to detect this.
    /// - We've just stepped to another injected breakpoint. In this case we
    ///   look whether our sentinel `.ark_auto_step()` was emitted by R as part
    ///   of the `Debug at` output.
    ///
    /// Returns `Some(ConsoleResult::NewInput)` if auto-stepping, `None` otherwise.
    ///
    /// TODO: Should set a flag in the Console state to prevent WriteConsole
    /// emission during these intermediate states.
    fn maybe_auto_step(&mut self, buf: *mut c_uchar, buflen: c_int) -> Option<ConsoleResult> {
        // Did we just step onto an injected call (breakpoint or verify)?
        let at_auto_step = matches!(
            &self.debug_call_text,
            DebugCallText::Finalized(text, DebugCallTextKind::DebugAt)
                if text.trim_start().starts_with("base::.ark_auto_step")
        );

        // Are we stopped by an injected breakpoint
        let in_injected_breakpoint = harp::r_current_function().inherits("ark_breakpoint");

        if in_injected_breakpoint || at_auto_step {
            let kind = if in_injected_breakpoint { "in" } else { "at" };
            log::trace!("Auto-step expression reached ({kind}), moving to next expression");

            self.debug_transient_eval = false;

            Self::on_console_input(buf, buflen, String::from("n")).unwrap();
            return Some(ConsoleResult::NewInput);
        }

        None
    }

    /// Invoked by R to write output to the console.
    fn write_console(buf: *const c_char, _buflen: i32, otype: i32) {
        let console = Console::get_mut();

        if let Some(captured) = &mut console.captured_output {
            captured.push_str(&console_to_utf8(buf).unwrap());
            return;
        }

        let content = match console_to_utf8(buf) {
            Ok(content) => content,
            Err(err) => panic!("Failed to read from R buffer: {err:?}"),
        };

        if !Console::is_initialized() {
            // During init, consider all output to be part of the startup banner
            match console.banner.as_mut() {
                Some(banner) => banner.push_str(&content),
                None => console.banner = Some(content),
            }
            return;
        }

        // To capture the current `debug: <call>` output, for use in the debugger's
        // match based fallback
        console.debug_handle_write_console(&content);

        let stream = if otype == 0 {
            Stream::Stdout
        } else {
            Stream::Stderr
        };

        // If active execution request is silent don't broadcast
        // any output
        if let Some(ref req) = console.active_request {
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
            if console.pending_inputs.is_none() {
                console.autoprint_output.push_str(&content);
                return;
            }

            // In notebooks, we don't emit results of intermediate expressions
            if console.session_mode == SessionMode::Notebook {
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
        console.iopub_tx.send(message).unwrap();
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

        // Compute busy state
        let busy = which != 0;

        // Send updated state to the frontend over the UI comm
        self.with_ui_comm_tx(|ui_comm_tx| {
            ui_comm_tx.send_event(UiFrontendEvent::Busy(BusyParams { busy }));
        });
    }

    /// Invoked by R to show a message to the user.
    fn show_message(&self, buf: *const c_char) {
        let message = unsafe { CStr::from_ptr(buf) };
        let message = message.to_str().unwrap().to_string();

        // Deliver message to the frontend over the UI comm
        self.with_ui_comm_tx(|ui_comm_tx| {
            ui_comm_tx.send_event(UiFrontendEvent::ShowMessage(ShowMessageParams { message }))
        });
    }

    /// Invoked by the R event loop
    fn polled_events(&mut self) {
        // Don't process tasks until R is fully initialized
        if !Self::is_initialized() {
            if !self.tasks_interrupt_rx.is_empty() {
                log::trace!("Delaying execution of interrupt task as R isn't initialized yet");
            }
            return;
        }

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

    fn process_idle_events() {
        // Process regular R events. We're normally running with polled
        // events disabled so that won't run here. We also run with
        // interrupts disabled, so on Windows those won't get run here
        // either (i.e. if `UserBreak` is set), but it will reset `UserBreak`
        // so we need to ensure we handle interrupts right before calling
        // this.
        unsafe { R_ProcessEvents() };

        crate::sys::console::run_activity_handlers();

        // Run pending finalizers. We need to do this eagerly as otherwise finalizers
        // might end up being executed on the LSP thread.
        // https://github.com/rstudio/positron/issues/431
        unsafe { R_RunPendingFinalizers() };

        // Check for Positron render requests.
        //
        // TODO: This should move to a spawned task that'd be woken up by
        // incoming messages on plot comms. This way we'll prevent the delays
        // introduced by timeout-based event polling.
        graphics_device::on_process_idle_events();
    }

    pub fn get_comm_event_tx(&self) -> &Sender<CommEvent> {
        &self.comm_event_tx
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

    fn send_lsp_notification(&mut self, event: KernelNotification) {
        log::trace!(
            "Sending LSP notification: {event:#?}",
            event = event.trace()
        );

        let Some(ref tx) = self.lsp_events_tx else {
            log::trace!("Failed to send LSP notification. LSP events channel isn't open yet, or has been closed. Event: {event:?}", event = event.trace());
            return;
        };

        if let Err(err) = tx.send(Event::Kernel(event)) {
            log::error!(
                "Failed to send LSP notification. Removing LSP events channel. Error: {err:?}"
            );
            self.remove_lsp_channel();
        }
    }

    pub(crate) fn set_lsp_channel(&mut self, lsp_events_tx: TokioUnboundedSender<Event>) {
        self.lsp_events_tx = Some(lsp_events_tx.clone());

        // Refresh LSP state now since we probably have missed some updates
        // while the channel was offline. This is currently not an ideal timing
        // as the channel is set up from a preemptive `r_task()` after the LSP
        // is set up. We'll want to do this in an idle task.
        log::trace!("LSP channel opened. Refreshing state.");
        self.refresh_lsp();
        self.notify_lsp_of_known_virtual_documents();
    }

    pub(crate) fn remove_lsp_channel(&mut self) {
        self.lsp_events_tx = None;
    }

    fn refresh_lsp(&mut self) {
        match console_inputs() {
            Ok(inputs) => {
                self.send_lsp_notification(KernelNotification::DidChangeConsoleInputs(inputs));
            },
            Err(err) => log::error!("Can't retrieve console inputs: {err:?}"),
        }
    }

    fn notify_lsp_of_known_virtual_documents(&mut self) {
        // Clone the whole HashMap since we need to own the uri/contents to send them
        // over anyways. We don't want to clear the map in case the LSP restarts later on
        // and we need to send them over again.
        let virtual_documents = self.lsp_virtual_documents.clone();

        for (uri, contents) in virtual_documents {
            self.send_lsp_notification(KernelNotification::DidOpenVirtualDocument(
                DidOpenVirtualDocumentParams { uri, contents },
            ))
        }
    }

    pub fn insert_virtual_document(&mut self, uri: String, contents: String) {
        log::trace!("Inserting vdoc for `{uri}`");

        // Strip scheme if any. We're only storing the path.
        let uri = uri.strip_prefix("ark:").unwrap_or(&uri).to_string();

        // Save our own copy of the virtual document. If the LSP is currently closed
        // or restarts, we can notify it of all virtual documents it should know about
        // in the LSP channel setup step. It is common for the kernel to create the
        // virtual documents for base R packages before the LSP has started up.
        self.lsp_virtual_documents
            .insert(uri.clone(), contents.clone());

        self.send_lsp_notification(KernelNotification::DidOpenVirtualDocument(
            DidOpenVirtualDocumentParams { uri, contents },
        ))
    }

    pub fn remove_virtual_document(&mut self, uri: String) {
        log::trace!("Removing vdoc for `{uri}`");

        // Strip scheme if any. We're only storing the path.
        let uri = uri.strip_prefix("ark:").unwrap_or(&uri).to_string();

        self.lsp_virtual_documents.remove(&uri);

        self.send_lsp_notification(KernelNotification::DidCloseVirtualDocument(
            DidCloseVirtualDocumentParams { uri },
        ))
    }

    pub fn has_virtual_document(&self, uri: &String) -> bool {
        let uri = uri.strip_prefix("ark:").unwrap_or(&uri).to_string();
        self.lsp_virtual_documents.contains_key(&uri)
    }

    pub fn get_virtual_document(&self, uri: &str) -> Option<String> {
        let uri = uri.strip_prefix("ark:").unwrap_or(uri);
        self.lsp_virtual_documents.get(uri).cloned()
    }

    pub fn call_frontend_method(&self, request: UiFrontendRequest) -> anyhow::Result<RObject> {
        log::trace!("Calling frontend method {request:?}");

        let ui_comm_tx = self.get_ui_comm_tx().ok_or_else(|| {
            anyhow::anyhow!("UI comm is not connected. Can't execute request {request:?}")
        })?;

        let (reply_tx, reply_rx) = bounded(1);

        let Some(req) = &self.active_request else {
            return Err(anyhow::anyhow!(
                "No active request. Can't execute request {request:?}"
            ));
        };

        // Forward request to UI comm
        ui_comm_tx.send_request(UiCommFrontendRequest {
            originator: req.originator.clone(),
            reply_tx,
            request: request.clone(),
        });

        // Block for reply
        let reply = reply_rx.recv().unwrap();

        log::trace!("Got reply from frontend method: {reply:?}");

        match reply {
            StdInRpcReply::Reply(reply) => match reply {
                JsonRpcReply::Result(reply) => {
                    // Deserialize to Rust first to verify the OpenRPC contract.
                    // Errors are propagated to R.
                    if let Err(err) = ui_frontend_reply_from_value(reply.result.clone(), &request) {
                        return Err(anyhow::anyhow!(
                            "Can't deserialize RPC reply for {request:?}:\n{err:?}"
                        ));
                    }

                    // Now deserialize to an R object
                    Ok(RObject::try_from(reply.result)?)
                },
                JsonRpcReply::Error(reply) => {
                    let message = reply.error.message;

                    return Err(anyhow::anyhow!(
                        "While calling frontend method:\n\
                         {message}",
                    ));
                },
            },
            // If an interrupt was signalled, return `NULL`. This should not be
            // visible to the caller since `r_unwrap()` (called e.g. by
            // `harp::register`) will trigger an interrupt jump right away.
            StdInRpcReply::Interrupt => Ok(RObject::null()),
        }
    }

    pub(crate) fn read_console_env(&self) -> RObject {
        self.read_console_env_stack
            .borrow()
            .last()
            .cloned()
            .unwrap_or_else(|| R_ENVS.global.into())
    }

    fn do_read_console_entry(&mut self, env: RObject) {
        self.read_console_depth
            .set(self.read_console_depth.get() + 1);

        self.read_console_nested_return.set(false);
        self.read_console_threw_error.set(true);

        self.read_console_env_stack.borrow_mut().push(env);
    }

    fn do_read_console_exit(&mut self) {
        self.read_console_depth
            .set(self.read_console_depth.get() - 1);

        self.read_console_env_stack.borrow_mut().pop();

        self.read_console_nested_return.set(true);

        // Always stop debug session when yielding back to R. This prevents
        // the debug toolbar from lingering in situations like:
        //
        // ```r
        // { local(browser()); Sys.sleep(10) }
        // ```
        //
        // For a more practical example see Shiny app example in
        // https://github.com/rstudio/rstudio/pull/14848
        self.debug_stop();
    }

    pub(crate) fn set_debug_selected_frame_id(&self, frame_id: Option<i64>) {
        self.debug_selected_frame_id.set(frame_id);
    }

    /// Check if this is a browser prompt for which we need to capture the
    /// evaluation environment
    fn needs_browser_capture(&self, prompt: *const c_char) -> bool {
        let prompt_str = unsafe { std::ffi::CStr::from_ptr(prompt) }.to_string_lossy();
        let is_browser = RE_DEBUG_PROMPT.is_match(&prompt_str);

        // Skip capture if there's a pending error, we need `read_console()` to
        // process it via `take_exception()` first.
        is_browser && self.last_error.is_none()
    }
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

/// Data passed to the eval body callback via `R_withCallingErrorHandler`.
#[repr(C)]
struct EvalBodyData {
    expr: libr::SEXP,
    frame: libr::SEXP,
}

/// Body callback for `R_withCallingErrorHandler` in `Console::eval`.
/// Simply evaluates the expression in the given frame.
unsafe extern "C-unwind" fn eval_body_callback(data: *mut c_void) -> libr::SEXP {
    let data = unsafe { &*(data as *const EvalBodyData) };
    unsafe { libr::Rf_eval(data.expr, data.frame) }
}

/// Error handler callback for `R_withCallingErrorHandler` in `Console::eval`.
/// This fires when an error occurs during evaluation in a debug REPL.
///
/// Calls the R-side `local_error_handler` which delegates to
/// `globalErrorHandler`. If error exception breakpoints are enabled, that
/// enters the error browser first. In all cases it saves the traceback
/// and invokes the abort restart to jump to top level.
unsafe extern "C-unwind" fn eval_error_callback(err: libr::SEXP, _data: *mut c_void) -> libr::SEXP {
    // Call the R-side global error handler which sets the stopped reason,
    // calls `browser()`, saves the backtrace, and invokes the `abort` or
    // `browser` restart.
    unsafe {
        let call = libr::Rf_lang2(r_symbol!(".ps.errors.globalErrorHandler"), err);
        libr::Rf_protect(call);
        libr::Rf_eval(call, ARK_ENVS.positron_ns);
    }

    unreachable!("globalErrorHandler longjumps via invokeRestart")
}

// --- Frontend methods ---
// These functions are hooked up as R frontend methods. They call into our
// global `Console` singleton.

#[cfg_attr(not(test), no_mangle)]
pub extern "C-unwind" fn r_read_console(
    prompt: *const c_char,
    buf: *mut c_uchar,
    buflen: c_int,
    hist: c_int,
) -> i32 {
    // In this entry point we handle two kinds of state:
    // - The number of nested REPLs `read_console_depth`
    // - A bunch of flags that help us reset the calling R REPL
    //
    // The second kind is unfortunate and due to us taking charge of parsing and
    // evaluation. Ideally R would extend their frontend API so that this would
    // only be necessary for backward compatibility with old versions of R.

    let console = Console::get_mut();

    // Propagate an EOF event (e.g. from a Shutdown request). We need to exit
    // from all consoles on the stack to let R shut down with an `exit()`.
    if console.read_console_shutdown.get() {
        return 0;
    }

    // Handle any pending action from a previous `r_read_console` call.
    // These are multi-step operations that required returning control to R.
    let env: RObject = match console.read_console_pending_action.take() {
        ReadConsolePendingAction::None => {
            // Check if this is a browser prompt that needs environment capture.
            // If so, return capture call WITHOUT doing any entry bookkeeping.
            if console.needs_browser_capture(prompt) {
                console
                    .read_console_pending_action
                    .set(ReadConsolePendingAction::CaptureEnv);

                // For browser REPLs, we capture the top-level environment by
                // returning an expression to R that basically does
                // `parent.frame()` and store it in a base symbol. There is no
                // way to reliably get this environment via regular evaluation:
                //
                // - Evaluating requires supplying an environment, which
                //   interferes with approaches based on `parent.frame()`.
                // - Looking at the call stack via `sys.frames()` does not work
                //   when the browser is evaluating a promise or some other
                //   C-level `Rf_eval()`.
                let input = String::from("base::.ark_capture_top_level_environment()");
                Console::on_console_input(buf, buflen, input).unwrap();
                return 1;
            }

            // At top-level: Use global env
            R_ENVS.global.into()
        },

        ReadConsolePendingAction::ExecuteInput(next_input) => {
            // We've finished evaluating a dummy value to reset state in R's REPL,
            // and are now ready to evaluate the actual input.
            Console::on_console_input(buf, buflen, next_input).unwrap();
            return 1;
        },

        ReadConsolePendingAction::CaptureEnv => {
            // We just evaluated `.ark_capture_top_level_environment()`.
            // Retrieve the captured environment from base namespace for entry
            // bookkeeping below.
            unsafe {
                let sym = r_symbol!(".ark_top_level_env");
                let env: RObject = libr::CDR(sym).into();

                // Allow R to GC the environment again
                libr::SETCDR(sym, libr::R_NilValue);

                if r_typeof(env.sexp) == libr::ENVSXP {
                    env
                } else {
                    log::warn!("Failed to capture browser environment, falling back");
                    harp::r_current_frame()
                }
            }
        },
    };

    // In case of error, we haven't had a chance to evaluate ".ark_last_value".
    // So we return to the R REPL to give R a chance to run the state
    // restoration that occurs between `R_ReadConsole()` and `eval()`:
    // - R_PPStackTop: https://github.com/r-devel/r-svn/blob/74cd0af4/src/main/main.c#L227
    // - R_EvalDepth:  https://github.com/r-devel/r-svn/blob/74cd0af4/src/main/main.c#L260
    //
    // Technically this also resets time limits (see `base::setTimeLimit()`) but
    // these aren't supported in Ark because they cause errors when we poll R
    // events.
    if console.last_error.is_some() && console.read_console_threw_error.get() {
        console.read_console_threw_error.set(false);

        // Evaluate last value so that `base::.Last.value` remains the same
        Console::on_console_input(
            buf,
            buflen,
            String::from("base::invisible(base::.Last.value)"),
        )
        .unwrap();
        return 1;
    }

    // Entry bookkeeping: increment depth, set flags, push frame.
    // Cleanup happens in the exit branch of `exec_with_cleanup()`.
    console.do_read_console_entry(env);

    exec_with_cleanup(
        || {
            let console = Console::get_mut();
            let result = r_read_console_impl(console, prompt, buf, buflen, hist);

            // If we get here, there was no error
            console.read_console_threw_error.set(false);

            result
        },
        || {
            Console::get_mut().do_read_console_exit();
        },
    )
}

fn r_read_console_impl(
    console: &mut Console,
    prompt: *const c_char,
    buf: *mut c_uchar,
    buflen: c_int,
    hist: c_int,
) -> i32 {
    let result = r_sandbox(|| console.read_console(prompt, buf, buflen, hist));

    let result = unwrap!(result, Err(err) => {
        panic!("Unexpected longjump while reading from console: {err:?}");
    });

    // NOTE: Keep this function a "Plain Old Frame" without any
    // destructors. We're longjumping from here in case of interrupt.

    match result {
        ConsoleResult::NewPendingInput(input) => {
            let PendingInput { expr, srcref } = input;

            unsafe {
                // The pointer protection stack is restored by `run_Consoleloop()`
                // after a longjump to top-level, so it's safe to protect here
                // even if the evaluation throws
                let expr = libr::Rf_protect(expr.into());
                let srcref = libr::Rf_protect(srcref.into());

                console.eval(expr, srcref, buf, buflen, console.debug_is_debugging);

                // Check if a nested read_console() just returned. If that's the
                // case, we need to reset the `R_ConsoleIob` by first returning
                // a dummy value causing a `PARSE_NULL` event.
                if console.read_console_nested_return.get() {
                    let next_input = Console::console_input(buf, buflen);
                    console
                        .read_console_pending_action
                        .set(ReadConsolePendingAction::ExecuteInput(next_input));

                    // Evaluating a space causes a `PARSE_NULL` event. Don't
                    // evaluate a newline, that would cause a parent debug REPL
                    // to interpret it as `n`, causing it to exit instead of
                    // being a no-op.
                    Console::on_console_input(buf, buflen, String::from(" ")).unwrap();
                    console.read_console_nested_return.set(false);
                }

                // We verify breakpoints _after_ evaluation is complete. An
                // error will prevent verification.
                console.verify_breakpoints(RObject::from(srcref));

                libr::Rf_unprotect(2);
                return 1;
            }
        },

        ConsoleResult::NewInput => {
            return 1;
        },

        ConsoleResult::Disconnected => {
            // Cause parent consoles to shutdown too
            console.read_console_shutdown.set(true);
            return 0;
        },

        ConsoleResult::Interrupt => {
            log::trace!("Interrupting `ReadConsole()`");
            unsafe {
                Rf_onintr();
            }

            // This normally does not return
            log::error!("`Rf_onintr()` did not longjump");
            return 0;
        },

        ConsoleResult::Error(message) => {
            // Save error message in `Console` to avoid leaking memory when
            // `Rf_error()` jumps. Some gymnastics are required to deal with the
            // possibility of `CString` conversion failure since the error
            // message comes from the frontend and might be corrupted.
            console.r_error_buffer = Some(new_cstring(message));
            unsafe { Rf_error(console.r_error_buffer.as_ref().unwrap().as_ptr()) }
        },
    };
}

fn new_cstring(x: String) -> CString {
    CString::new(x).unwrap_or(CString::new("Can't create CString").unwrap())
}

#[cfg_attr(not(test), no_mangle)]
pub extern "C-unwind" fn r_write_console(buf: *const c_char, buflen: i32, otype: i32) {
    if let Err(err) = r_sandbox(|| Console::write_console(buf, buflen, otype)) {
        panic!("Unexpected longjump while writing to console: {err:?}");
    };
}

#[cfg_attr(not(test), no_mangle)]
pub extern "C-unwind" fn r_show_message(buf: *const c_char) {
    Console::get().show_message(buf);
}

#[cfg_attr(not(test), no_mangle)]
pub extern "C-unwind" fn r_busy(which: i32) {
    Console::get_mut().busy(which);
}

#[cfg_attr(not(test), no_mangle)]
pub extern "C-unwind" fn r_suicide(buf: *const c_char) {
    let msg = unsafe { CStr::from_ptr(buf) };
    panic!("Suicide: {}", msg.to_str().unwrap());
}

#[cfg_attr(not(test), no_mangle)]
pub unsafe extern "C-unwind" fn r_polled_events() {
    if let Err(err) = r_sandbox(|| Console::get_mut().polled_events()) {
        panic!("Unexpected longjump while polling events: {err:?}");
    };
}

// This hook is called like a user onLoad hook but for every package to be
// loaded in the session
#[harp::register]
unsafe extern "C-unwind" fn ps_onload_hook(pkg: SEXP, _path: SEXP) -> anyhow::Result<SEXP> {
    // NOTE: `_path` might be NULL for a compat reason, see comments on the R side

    let pkg: String = RObject::view(pkg).try_into()?;

    // Need to reset parent as this might run in the context of another thread's R task
    let _span = tracing::trace_span!(parent: None, "onload_hook", pkg = pkg).entered();

    // Populate fake source refs if needed
    if do_resource_namespaces() {
        r_task::spawn_idle(|_| async move {
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

    // By default we don't eagerly resource namespaces to avoid increased memory usage.
    // https://github.com/posit-dev/positron/issues/5050
    opt.unwrap_or(false)
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

    // The call-stack is empty so this must be R auto-printing an unclassed
    // object. Note that this might wrongly return true in debug REPLs. Ideally
    // we'd take note of the number of frames on the stack when we enter
    // `r_read_console()`, and compare against that.
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

#[harp::register]
unsafe extern "C-unwind" fn ps_insert_virtual_document(
    uri: SEXP,
    contents: SEXP,
) -> anyhow::Result<SEXP> {
    let uri: String = RObject::view(uri).try_into()?;
    let contents: String = RObject::view(contents).try_into()?;

    Console::get_mut().insert_virtual_document(uri, contents);

    Ok(RObject::null().sexp)
}

#[harp::register]
unsafe extern "C-unwind" fn ps_get_virtual_document(uri: SEXP) -> anyhow::Result<SEXP> {
    let uri: String = RObject::view(uri).try_into()?;

    let content = Console::get().get_virtual_document(&uri);

    match content {
        Some(content) => Ok(RObject::from(content).sexp),
        None => Ok(RObject::null().sexp),
    }
}
