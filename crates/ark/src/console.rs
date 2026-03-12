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
use harp::utils::r_poke_option;
use harp::utils::r_typeof;
use harp::CONSOLE_THREAD_ID;
use libr::R_BaseNamespace;
use libr::R_GlobalEnv;
use libr::R_ProcessEvents;
use libr::R_RunPendingFinalizers;
use libr::Rf_ScalarInteger;
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
use uuid::Uuid;

mod console_annotate;
mod console_comm;
mod console_debug;
mod console_error;
mod console_filter;
mod console_integration;
mod console_repl;

use console_annotate::annotate_input;
pub(crate) use console_debug::DebugCallText;
pub(crate) use console_debug::DebugStoppedReason;
pub(crate) use console_debug::FrameInfo;
use console_debug::FrameInfoId;
pub(crate) use console_debug::FrameSource;
use console_error::stack_overflow_occurred;
use console_filter::strip_step_lines;
use console_filter::ConsoleFilter;
pub(crate) use console_repl::console_inputs;
pub(crate) use console_repl::r_busy;
pub(crate) use console_repl::r_polled_events;
pub(crate) use console_repl::r_read_console;
pub(crate) use console_repl::r_show_message;
pub(crate) use console_repl::r_suicide;
pub(crate) use console_repl::r_write_console;
pub(crate) use console_repl::selected_env;
use console_repl::ActiveReadConsoleRequest;
pub(crate) use console_repl::ConsoleNotification;
pub(crate) use console_repl::ConsoleOutputCapture;
pub(crate) use console_repl::KernelInfo;
use console_repl::PendingInputs;
use console_repl::PromptInfo;
use console_repl::ReadConsolePendingAction;
pub use console_repl::SessionMode;

use crate::comm_handler::ConsoleComm;
use crate::comm_handler::EnvironmentChanged;
use crate::dap::dap_state::Breakpoint;
use crate::dap::Dap;
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
use crate::r_task::QueuedRTask;
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
use crate::url::UrlId;

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

pub(crate) struct Console {
    pub(crate) positron_ns: Option<RObject>,

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

    /// Channel to send and receive tasks from `QueuedRTask`s
    tasks_interrupt_rx: Receiver<QueuedRTask>,
    tasks_idle_rx: Receiver<QueuedRTask>,
    tasks_idle_any_rx: Receiver<QueuedRTask>,
    pending_futures: HashMap<Uuid, (BoxFuture<'static, ()>, RTaskStartInfo, Option<String>)>,

    /// Channel to communicate requests and events to the frontend
    /// by forwarding them through the UI comm. Optional, and really Positron specific.
    ui_comm_tx: Option<UiCommSender>,

    /// Error captured by our global condition handler during the last iteration
    /// of the REPL.
    last_error: Option<Exception>,

    /// Channel to communicate with the Help thread
    help_event_tx: Option<Sender<HelpEvent>>,
    /// R help port
    help_port: Option<u16>,

    /// Event channel for notifying the LSP. In principle, could be a Jupyter comm.
    lsp_events_tx: Option<TokioUnboundedSender<Event>>,

    /// The kernel's copy of virtual documents to notify the LSP about when the LSP
    /// initially connects and after an LSP restart.
    lsp_virtual_documents: HashMap<String, String>,

    pending_inputs: Option<PendingInputs>,

    /// Banner output accumulated during startup, but set to `None` after we complete
    /// the initialization procedure and forward the banner on
    banner: Option<String>,

    /// Raw error buffer provided to `Rf_error()` when throwing `r_read_console()` errors.
    /// Stored in `Console` to avoid memory leakage when `Rf_error()` jumps.
    r_error_buffer: Option<CString>,

    /// When `Some`, console output is captured here instead of being sent to IOPub.
    /// Interact with this via `ConsoleOutputCapture` from `start_capture()`.
    captured_output: Option<String>,

    /// Whether the current evaluation is transient within the debug session.
    /// When `true`, the debug session state is preserved: no Continued/Stopped
    /// events are emitted, frame IDs remain valid, and only an Invalidated
    /// event is sent to refresh variables. Set to `true` for console
    /// evaluations (as opposed to step commands like `n`, `c`, `f`).
    /// See https://github.com/posit-dev/positron/issues/3151.
    debug_transient_eval: bool,

    /// Underlying dap state. Shared with the DAP server thread.
    debug_dap: Arc<Mutex<Dap>>,

    /// Whether or not we are currently in a debugging state.
    debug_is_debugging: bool,

    /// Filter for debug console output. Removes R's internal debug messages
    /// from user-visible console output.
    debug_filter: ConsoleFilter,

    /// The current call emitted by R as `debug: <call-text>`.
    debug_call_text: Option<DebugCallText>,

    /// The last known `start_line` for the active context frame.
    debug_last_line: Option<i64>,

    /// The stack of frames we saw the last time we stopped. Used as a mostly
    /// reliable indication of whether we moved since last time.
    debug_last_stack: Vec<FrameInfoId>,

    /// Ever increasing debug session index. Used to create URIs that are only
    /// valid for a single session.
    debug_session_index: u32,

    /// The current frame `id`. Monotonically increasing, unique across all
    /// frames and debug sessions. It's important that each frame gets a unique
    /// ID across the process lifetime so that we can invalidate stale requests.
    debug_current_frame_id: i64,

    /// Reason for entering the debugger. Used to determine which DAP event to send.
    debug_stopped_reason: Option<DebugStoppedReason>,

    /// The frame ID selected by the user in the debugger UI.
    /// When set, console evaluations happen in this frame's environment instead of the current frame.
    /// Resolved to an environment via `debug_dap` state when needed.
    debug_selected_frame_id: Cell<Option<i64>>,

    /// Saved JIT compiler level, to restore after a step-into command.
    /// Step-into disables JIT to prevent stepping into `compiler` internals.
    debug_jit_level: Option<i32>,

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
    read_console_env_stack: RefCell<Vec<RObject>>,

    /// Comm handlers registered on the R thread (keyed by comm ID).
    comms: HashMap<String, ConsoleComm>,
}
