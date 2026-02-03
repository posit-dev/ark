use std::io::Write;
use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::time::Duration;

use amalthea::fixtures::dummy_frontend::DummyConnection;
use amalthea::fixtures::dummy_frontend::DummyFrontend;
use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::comm_open::CommOpen;
use amalthea::wire::execute_request::ExecuteRequestPositron;
use amalthea::wire::execute_request::JupyterPositronLocation;
use amalthea::wire::execute_request::JupyterPositronPosition;
use amalthea::wire::execute_request::JupyterPositronRange;
use amalthea::wire::jupyter_message::Message;
use ark::console::SessionMode;
use ark::repos::DefaultRepos;
use ark::url::ExtUrl;
use tempfile::NamedTempFile;

use crate::execute_result_contains;
use crate::is_idle;
use crate::is_start_debug;
use crate::is_stop_debug;
use crate::is_stream;
use crate::stream_contains;
use crate::tracing::trace_separator;
use crate::tracing::trace_shell_reply;
use crate::tracing::trace_shell_request;
use crate::DapClient;
use crate::MessageAccumulator;

// There can be only one frontend per process. Needs to be in a mutex because
// the frontend wraps zmq sockets which are unsafe to send across threads.
//
// This is using `OnceLock` because it provides a way of checking whether the
// value has been initialized already. Also we'll need to parameterize
// initialization in the future.
static FRONTEND: OnceLock<Arc<Mutex<DummyFrontend>>> = OnceLock::new();

/// Wrapper around `DummyFrontend` that checks sockets are empty on drop
pub struct DummyArkFrontend {
    guard: MutexGuard<'static, DummyFrontend>,
}

struct DummyArkFrontendOptions {
    interactive: bool,
    site_r_profile: bool,
    user_r_profile: bool,
    r_environ: bool,
    session_mode: SessionMode,
    default_repos: DefaultRepos,
    startup_file: Option<String>,
}

/// Wrapper around `DummyArkFrontend` that uses `SessionMode::Notebook`
///
/// Only one of `DummyArkFrontend` or `DummyArkFrontendNotebook` can be used in
/// a given process. Just don't import both and you should be fine as Rust will
/// let you know about a missing symbol if you happen to copy paste `lock()`
/// calls of different kernel types between files.
pub struct DummyArkFrontendNotebook {
    inner: DummyArkFrontend,
}

/// Wrapper around `DummyArkFrontend` that allows an `.Rprofile` to run
pub struct DummyArkFrontendRprofile {
    inner: DummyArkFrontend,
}

/// Wrapper around `DummyArkFrontend` that allows setting default repos
/// for the frontend
pub struct DummyArkFrontendDefaultRepos {
    inner: DummyArkFrontend,
}

impl DummyArkFrontend {
    pub fn lock() -> Self {
        Self {
            guard: Self::get_frontend().lock().unwrap(),
        }
    }

    /// Wait for R cleanup to start (indicating shutdown has been initiated).
    /// Panics if cleanup doesn't start within the timeout.
    #[cfg(unix)]
    #[track_caller]
    pub fn wait_for_cleanup() {
        use std::time::Duration;

        use ark::sys::console::CLEANUP_SIGNAL;

        let (lock, cvar) = &CLEANUP_SIGNAL;
        let result = cvar
            .wait_timeout_while(lock.lock().unwrap(), Duration::from_secs(3), |started| {
                !*started
            })
            .unwrap();

        if !*result.0 {
            panic!("Cleanup did not start within timeout");
        }
    }

    /// Start DAP server via comm protocol and return a connected client.
    ///
    /// This sends a `comm_open` message to start the DAP server, waits for
    /// the `server_started` response with the port, and connects a `DapClient`.
    #[track_caller]
    pub fn start_dap(&self) -> DapClient {
        let comm_id = uuid::Uuid::new_v4().to_string();

        // Send comm_open to start the DAP server
        self.send_shell(CommOpen {
            comm_id: comm_id.clone(),
            target_name: String::from("ark_dap"),
            data: serde_json::json!({ "ip_address": "127.0.0.1" }),
        });

        // Message order: Busy, then CommMsg and Idle in either order.
        // The CommMsg travels through an async path (comm_socket -> comm manager -> iopub)
        // while Idle is sent directly to iopub_tx, so they may arrive out of order.
        // See FIXME notes at https://github.com/posit-dev/ark/issues/689
        self.recv_iopub_busy();

        let mut port: Option<u16> = None;
        let mut got_idle = false;

        while port.is_none() || !got_idle {
            let msg = self.recv_iopub();
            match msg {
                Message::CommMsg(data) => {
                    assert_eq!(data.content.comm_id, comm_id);
                    let method = data.content.data["method"]
                        .as_str()
                        .expect("Expected method field");
                    assert_eq!(method, "server_started");
                    port = Some(
                        data.content.data["params"]["port"]
                            .as_u64()
                            .expect("Expected port field") as u16,
                    );
                },
                Message::Status(status) => {
                    use amalthea::wire::status::ExecutionState;
                    if status.content.execution_state == ExecutionState::Idle {
                        got_idle = true;
                    }
                },
                other => panic!("Expected CommMsg or Status(Idle), got {:?}", other),
            }
        }

        let port = port.unwrap();

        let mut client = DapClient::connect("127.0.0.1", port).unwrap();
        client.initialize();
        client.attach();
        client
    }

    /// Receive exactly `n` iopub messages, returning a wrapper for inspection.
    ///
    /// Use this when multiple messages may arrive in non-deterministic order
    /// (e.g., from different threads sending to iopub concurrently).
    ///
    /// Use `pop()` to extract expected messages and `assert_all_consumed()` to
    /// verify no unexpected messages remain.
    #[track_caller]
    pub fn recv_iopub_n(&self, n: usize) -> UnorderedMessages {
        let mut messages = Vec::with_capacity(n);
        for _ in 0..n {
            messages.push(self.recv_iopub());
        }
        UnorderedMessages { messages }
    }

    /// Receive iopub messages until all predicates are matched.
    ///
    /// Messages may arrive in any order and through different async paths
    /// (CommManager for comm messages, Shell for status, R console for streams).
    /// This function keeps receiving until every predicate has matched exactly
    /// one message, with a maximum message count to prevent infinite loops.
    ///
    /// Panics if:
    /// - Timeout waiting for a message
    /// - Maximum message count reached without matching all predicates
    #[track_caller]
    pub fn recv_iopub_async(&self, mut predicates: Vec<Box<dyn FnMut(&Message) -> bool>>) {
        // Allow some extra messages beyond the predicate count to handle
        // stream splitting and race conditions
        let max_messages = predicates.len() + 10;
        let mut received: Vec<Message> = Vec::new();
        let predicate_count = predicates.len();

        while !predicates.is_empty() {
            if received.len() >= max_messages {
                panic!(
                    "Received {} messages without matching all predicates.\n\
                     Unmatched predicates: {}\n\
                     Received messages: {:#?}",
                    received.len(),
                    predicates.len(),
                    received
                );
            }

            // Try to receive with panic recovery to provide better diagnostics
            let recv_result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.recv_iopub()));

            let msg = match recv_result {
                Ok(msg) => msg,
                Err(_) => {
                    panic!(
                        "Timeout or error while waiting for IOPub message.\n\
                         Expected {} predicates, {} remaining unmatched.\n\
                         Received so far: {:#?}",
                        predicate_count,
                        predicates.len(),
                        received
                    );
                },
            };

            // Try to match this message against any remaining predicate
            let matched_idx = predicates.iter_mut().position(|p| p(&msg));

            if let Some(idx) = matched_idx {
                drop(predicates.remove(idx));
            }

            received.push(msg);
        }
    }

    /// Receive IOPub messages until a condition is satisfied.
    ///
    /// This is a convenient wrapper around `MessageAccumulator` that handles
    /// stream coalescing automatically. Stream messages with the same parent
    /// header are combined before checking the condition, making tests immune
    /// to whether R batched or split console output.
    ///
    /// After the condition is satisfied, any remaining messages are drained
    /// with a short timeout to prevent interference with subsequent operations.
    ///
    /// # Example
    ///
    /// ```ignore
    /// frontend.recv_iopub_until(|acc| {
    ///     acc.streams_contain("Called from:") &&
    ///     acc.has_comm_method("start_debug") &&
    ///     acc.saw_idle()
    /// });
    /// ```
    #[track_caller]
    pub fn recv_iopub_until<F>(&self, condition: F)
    where
        F: FnMut(&mut MessageAccumulator) -> bool,
    {
        let mut acc = MessageAccumulator::new();
        let result = acc.receive_until(&self.iopub_socket, condition, Duration::from_secs(10));

        if let Err(msg) = result {
            panic!("Timeout waiting for IOPub condition.\n{msg}");
        }
    }

    /// Send an execute request with tracing
    #[track_caller]
    pub fn send_execute_request_traced(&self, code: &str, options: ExecuteRequestOptions) {
        trace_shell_request("execute_request", Some(code));
        self.send_execute_request(code, options);
    }

    /// Receive shell execute reply with tracing
    #[track_caller]
    pub fn recv_shell_execute_reply_traced(&self) -> u32 {
        let result = self.recv_shell_execute_reply();
        trace_shell_reply("execute_reply", "ok");
        result
    }

    /// Source a file that was created with `SourceFile::new()`.
    #[track_caller]
    pub fn source_file(&self, file: &SourceFile) {
        trace_separator(&format!("source({})", file.filename));
        self.send_execute_request(
            &format!("source('{}')", file.path),
            ExecuteRequestOptions::default(),
        );
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_execute_result();
        self.recv_iopub_idle();
        self.recv_shell_execute_reply();
    }

    /// Execute code from a file with location information.
    ///
    /// This simulates running code from an editor where the frontend sends
    /// the file URI and position. Breakpoints in the code will be verified
    /// during execution.
    #[track_caller]
    pub fn execute_file(&self, file: &SourceFile) {
        let code = std::fs::read_to_string(&file.path).unwrap();
        self.send_execute_request(&code, ExecuteRequestOptions {
            positron: Some(ExecuteRequestPositron {
                code_location: Some(file.location()),
            }),
            ..Default::default()
        });
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();
        self.recv_iopub_idle();
        self.recv_shell_execute_reply();
    }

    /// Source a file and stop at a breakpoint (set via DAP, not browser() in code).
    ///
    /// Source a file and wait until execution stops at an injected breakpoint.
    ///
    /// Use this when you've set breakpoints via `dap.set_breakpoints()` before sourcing.
    /// The caller must still receive the DAP events (see below) and should call
    /// `recv_shell_execute_reply()` after quitting the debugger.
    ///
    /// Due to the auto-stepping mechanism, hitting an injected breakpoint produces
    /// IOPub messages that may arrive in varying order and batching:
    /// - Stream output with "Called from:" and "debug at" (may be batched or separate)
    /// - start_debug / stop_debug comm messages
    /// - idle status
    ///
    /// **DAP events (caller must receive):**
    /// 1. Stopped (entering .ark_breakpoint)
    /// 2. Continued (auto-step triggered)
    /// 3. Continued (from stop_debug)
    /// 4. Stopped (at actual user expression)
    #[track_caller]
    pub fn source_file_and_hit_breakpoint(&self, file: &SourceFile) {
        trace_separator(&format!("source_and_hit_bp({})", file.filename));
        self.send_execute_request(
            &format!("source('{}')", file.path),
            ExecuteRequestOptions::default(),
        );
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        // Auto-stepping message flow when hitting an injected breakpoint.
        // Message count varies due to stream batching and timing. We collect
        // messages until we have evidence of all required events.
        self.recv_iopub_breakpoint_hit();
    }

    /// Receive IOPub messages for a breakpoint hit, handling variable batching.
    ///
    /// Uses `MessageAccumulator` to coalesce stream fragments, making the test
    /// immune to whether R batched or split the output across messages.
    #[track_caller]
    pub fn recv_iopub_breakpoint_hit(&self) {
        self.recv_iopub_until(|acc| {
            acc.streams_contain("Called from:") &&
                acc.streams_contain("debug at") &&
                acc.has_comm_method_count("start_debug", 2) &&
                acc.has_comm_method("stop_debug") &&
                acc.saw_idle()
        });
    }

    /// Receive IOPub messages for a breakpoint hit from a direct function call.
    ///
    /// This is similar to `recv_iopub_breakpoint_hit` but for direct function calls
    /// (e.g., `foo()`) rather than `source()`. When the function has source references
    /// (from a file created with `SourceFile::new()`), R will print `"debug at"`.
    #[track_caller]
    pub fn recv_iopub_breakpoint_hit_direct(&self) {
        self.recv_iopub_until(|acc| {
            acc.streams_contain("Called from:") &&
                acc.streams_contain("debug at") &&
                acc.has_comm_method_count("start_debug", 2) &&
                acc.has_comm_method("stop_debug") &&
                acc.saw_idle()
        });
    }

    /// Source a file that was created with `SourceFile::new()`.
    ///
    /// The code must contain `browser()` or a breakpoint to enter debug mode.
    /// The caller must still receive the DAP `Stopped` event.
    #[track_caller]
    pub fn source_debug_file(&self, file: &SourceFile) {
        trace_separator(&format!("source_debug({})", file.filename));
        self.send_execute_request(
            &format!("source('{}')", file.path),
            ExecuteRequestOptions::default(),
        );
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        self.recv_iopub_async(vec![
            is_start_debug(),
            stream_contains("Called from:"),
            is_idle(),
        ]);

        self.recv_shell_execute_reply();
    }

    /// Source a file containing the given code and receive all expected messages.
    ///
    /// Returns a `SourcedFile` containing the temp file (which must be kept alive)
    /// and the filename for use in assertions.
    ///
    /// The caller must still receive the DAP `Stopped` event.
    #[track_caller]
    pub fn send_source(&self, code: &str) -> SourceFile {
        let line_count = code.lines().count() as u32;
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{code}").unwrap();

        let url = ExtUrl::from_file_path(file.path()).unwrap();
        let path = url.path().to_string();
        let uri = url.to_string();
        let filename = file
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        self.send_execute_request(
            &format!("source('{path}')"),
            ExecuteRequestOptions::default(),
        );
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        self.recv_iopub_async(vec![
            is_start_debug(),
            stream_contains("Called from:"),
            is_idle(),
        ]);

        self.recv_shell_execute_reply();

        SourceFile {
            _file: file,
            path,
            filename,
            uri,
            line_count,
        }
    }

    /// Execute `browser()` and receive all expected messages.
    #[track_caller]
    pub fn debug_send_browser(&self) -> u32 {
        self.send_execute_request("browser()", ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        // Receive 3 messages in non-deterministic order: start_debug, execute_result, idle.
        // Message ordering is non-deterministic because they originate from different
        // threads (comm manager vs shell handler) that both send to the iopub socket.
        self.recv_iopub_async(vec![
            is_start_debug(),
            execute_result_contains("Called from: top level"),
            is_idle(),
        ]);

        self.recv_shell_execute_reply()
    }

    /// Execute `Q` to quit the browser and receive all expected messages.
    #[track_caller]
    pub fn debug_send_quit(&self) -> u32 {
        self.send_execute_request("Q", ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        self.recv_iopub_async(vec![is_stop_debug(), is_idle()]);

        self.recv_shell_execute_reply()
    }

    /// Execute `n` (next/step over) and receive all expected messages.
    #[track_caller]
    pub fn debug_send_next(&self) -> u32 {
        self.debug_send_step("n")
    }

    /// Execute `s` (step in) and receive all expected messages.
    #[track_caller]
    pub fn debug_send_step_in(&self) -> u32 {
        self.debug_send_step("s")
    }

    /// Execute `f` (finish/step out) and receive all expected messages.
    #[track_caller]
    pub fn debug_send_finish(&self) -> u32 {
        self.debug_send_step("f")
    }

    /// Execute `c` (continue) and receive all expected messages.
    #[track_caller]
    pub fn debug_send_continue(&self) -> u32 {
        self.debug_send_step("c")
    }

    /// Execute `c` (continue) to next browser() breakpoint in a sourced file.
    ///
    /// When continuing from one browser() to another, R outputs "Called from:"
    /// instead of "debug at", so this needs a different message pattern.
    #[track_caller]
    pub fn debug_send_continue_to_breakpoint(&self) -> u32 {
        self.send_execute_request("c", ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        self.recv_iopub_async(vec![
            is_stop_debug(),
            is_start_debug(),
            stream_contains("Called from:"),
            is_idle(),
        ]);

        self.recv_shell_execute_reply()
    }

    /// Execute an expression while in debug mode and receive all expected messages.
    ///
    /// This is for evaluating expressions that don't advance the debugger (e.g., `1`, `x`).
    /// The caller must still receive the DAP `Stopped` event with `preserve_focus_hint=true`.
    #[track_caller]
    pub fn debug_send_expr(&self, expr: &str) -> u32 {
        self.send_execute_request(expr, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        self.recv_iopub_async(vec![
            is_stop_debug(),
            is_start_debug(),
            crate::is_execute_result(),
            is_idle(),
        ]);

        self.recv_shell_execute_reply()
    }

    /// Execute an expression that causes an error while in debug mode.
    ///
    /// Unlike stepping to an error (which exits debug), evaluating an error
    /// from the console should keep us in debug mode.
    /// The caller must still receive the DAP `Stopped` event with `preserve_focus_hint=true`.
    ///
    /// Note: In debug mode, errors are streamed on stderr (not as `ExecuteError`)
    /// and a regular execution reply is sent. That's a limitation of the R kernel.
    #[track_caller]
    pub fn debug_send_error_expr(&self, expr: &str) -> u32 {
        self.send_execute_request(expr, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        self.recv_iopub_async(vec![
            is_stop_debug(),
            is_start_debug(),
            is_stream(),
            is_idle(),
        ]);

        self.recv_shell_execute_reply()
    }

    /// Helper for debug step commands that continue execution.
    #[track_caller]
    fn debug_send_step(&self, cmd: &str) -> u32 {
        self.send_execute_request(cmd, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        self.recv_iopub_async(vec![is_start_debug(), is_idle()]);

        self.recv_shell_execute_reply()
    }

    /// Execute a step command in a sourced file context.
    ///
    /// In sourced files with srcrefs, stepping produces additional messages compared
    /// to virtual document context: a `stop_debug` comm (debug session ends briefly),
    /// and a `Stream` with "debug at" output from R.
    ///
    /// This helper only consumes IOPub and shell messages. The caller must still
    /// consume DAP events separately.
    #[track_caller]
    pub fn debug_send_step_command(&self, cmd: &str) -> u32 {
        trace_separator(&format!("debug_step({})", cmd));
        self.send_execute_request(cmd, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        self.recv_iopub_async(vec![
            is_stop_debug(),
            is_start_debug(),
            stream_contains("debug at"),
            is_idle(),
        ]);

        self.recv_shell_execute_reply()
    }
}

/// Result of sourcing a file via `send_source()`.
///
/// The temp file is kept alive as long as this struct exists.
pub struct SourceFile {
    _file: NamedTempFile,
    pub path: String,
    pub filename: String,
    uri: String,
    line_count: u32,
}

impl SourceFile {
    /// Create a temp file with the given code without sourcing it.
    ///
    /// Use this when you need to set breakpoints before sourcing.
    /// After setting breakpoints, call `frontend.source_file()` to run the file.
    pub fn new(code: &str) -> Self {
        // Count lines for the location range
        let line_count = code.lines().count() as u32;
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{code}").unwrap();

        let url = ExtUrl::from_file_path(file.path()).unwrap();
        let path = url.path().to_string();
        let uri = url.to_string();

        // Extract file name
        let filename = file
            .path()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        Self {
            _file: file,
            path,
            filename,
            uri,
            line_count,
        }
    }

    /// Get a `JupyterPositronLocation` pointing to this file.
    pub fn location(&self) -> JupyterPositronLocation {
        JupyterPositronLocation {
            uri: self.uri.clone(),
            range: JupyterPositronRange {
                start: JupyterPositronPosition {
                    line: 0,
                    character: 0,
                },
                end: JupyterPositronPosition {
                    line: self.line_count,
                    character: 0,
                },
            },
        }
    }
}

/// Wrapper for messages that may arrive in non-deterministic order.
///
/// Use `pop()` to extract expected messages and `assert_all_consumed()` to
/// verify no unexpected messages remain.
#[derive(Debug)]
pub struct UnorderedMessages {
    pub messages: Vec<Message>,
}

impl UnorderedMessages {
    /// Remove and return the first message matching the predicate.
    ///
    /// Panics if no message matches.
    #[track_caller]
    pub fn pop<F>(&mut self, mut predicate: F) -> Message
    where
        F: FnMut(&Message) -> bool,
    {
        let pos = self
            .messages
            .iter()
            .position(|m| predicate(m))
            .expect("No message matched the predicate");
        self.messages.remove(pos)
    }

    /// Assert that all messages have been consumed.
    ///
    /// Panics with details of remaining messages if any exist.
    #[track_caller]
    pub fn assert_all_consumed(self) {
        if !self.messages.is_empty() {
            panic!("Unexpected messages remaining: {:#?}", self.messages);
        }
    }
}

impl DummyArkFrontend {
    fn get_frontend() -> &'static Arc<Mutex<DummyFrontend>> {
        // These are the hard-coded defaults. Call `init()` explicitly to
        // override.
        let options = DummyArkFrontendOptions::default();
        FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyArkFrontend::init(options))))
    }

    fn init(options: DummyArkFrontendOptions) -> DummyFrontend {
        if FRONTEND.get().is_some() {
            panic!("Can't spawn Ark more than once");
        }

        // We don't want cli to try and restore the cursor, it breaks our tests
        // by adding unecessary ANSI escapes. We don't need this in Positron because
        // cli also checks `isatty(stdout())`, which is false in Positron because
        // we redirect stdout.
        // https://github.com/r-lib/cli/blob/1220ed092c03e167ff0062e9839c81d7258a4600/R/onload.R#L33-L40
        unsafe { std::env::set_var("R_CLI_HIDE_CURSOR", "false") };

        let connection = DummyConnection::new();
        let (connection_file, registration_file) = connection.get_connection_files();

        let mut r_args = vec![];

        // We aren't animals!
        r_args.push(String::from("--no-save"));
        r_args.push(String::from("--no-restore"));

        if options.interactive {
            r_args.push(String::from("--interactive"));
        }
        if !options.site_r_profile {
            r_args.push(String::from("--no-site-file"));
        }
        if !options.user_r_profile {
            r_args.push(String::from("--no-init-file"));
        }
        if !options.r_environ {
            r_args.push(String::from("--no-environ"));
        }

        // Start the kernel and REPL in a background thread, does not return and is never joined.
        // Must run `start_kernel()` in a background thread because it blocks until it receives
        // a `HandshakeReply`, which we send from `from_connection()` below.
        stdext::spawn!("dummy_kernel", move || {
            ark::start::start_kernel(
                connection_file,
                Some(registration_file),
                r_args,
                options.startup_file,
                options.session_mode,
                false,
                options.default_repos,
            );
        });

        DummyFrontend::from_connection(connection)
    }
}

// Check that we haven't left crumbs behind
impl Drop for DummyArkFrontend {
    fn drop(&mut self) {
        self.assert_no_incoming()
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyArkFrontend {
    type Target = DummyFrontend;

    fn deref(&self) -> &Self::Target {
        Deref::deref(&self.guard)
    }
}

impl DerefMut for DummyArkFrontend {
    fn deref_mut(&mut self) -> &mut Self::Target {
        DerefMut::deref_mut(&mut self.guard)
    }
}

impl DummyArkFrontendNotebook {
    /// Lock a notebook frontend.
    ///
    /// NOTE: Only one `DummyArkFrontend` variant should call `lock()` within
    /// a given process.
    pub fn lock() -> Self {
        Self::init();

        Self {
            inner: DummyArkFrontend::lock(),
        }
    }

    /// Initialize with Notebook session mode
    fn init() {
        let mut options = DummyArkFrontendOptions::default();
        options.session_mode = SessionMode::Notebook;
        FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyArkFrontend::init(options))));
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyArkFrontendNotebook {
    type Target = DummyFrontend;

    fn deref(&self) -> &Self::Target {
        Deref::deref(&self.inner)
    }
}

impl DerefMut for DummyArkFrontendNotebook {
    fn deref_mut(&mut self) -> &mut Self::Target {
        DerefMut::deref_mut(&mut self.inner)
    }
}

impl DummyArkFrontendDefaultRepos {
    /// Lock a frontend with a default repos setting.
    ///
    /// NOTE: `startup_file` is required because you typically want
    /// to force `options(repos =)` to a fixed value for testing, regardless
    /// of what the caller's default `repos` are set as (i.e. rig typically
    /// sets it to a non-`@CRAN@` value).
    ///
    /// NOTE: Only one `DummyArkFrontend` variant should call `lock()` within
    /// a given process.
    pub fn lock(default_repos: DefaultRepos, startup_file: String) -> Self {
        Self::init(default_repos, startup_file);

        Self {
            inner: DummyArkFrontend::lock(),
        }
    }

    /// Initialize with given default repos
    fn init(default_repos: DefaultRepos, startup_file: String) {
        let mut options = DummyArkFrontendOptions::default();
        options.default_repos = default_repos;
        options.startup_file = Some(startup_file);

        FRONTEND.get_or_init(|| Arc::new(Mutex::new(DummyArkFrontend::init(options))));
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyArkFrontendDefaultRepos {
    type Target = DummyFrontend;

    fn deref(&self) -> &Self::Target {
        Deref::deref(&self.inner)
    }
}
impl DummyArkFrontendRprofile {
    /// Lock a frontend that supports `.Rprofile`s.
    ///
    /// NOTE: This variant can only be called exactly once per process,
    /// because you can only load an `.Rprofile` one time. Additionally,
    /// only one `DummyArkFrontend` variant should call `lock()` within
    /// a given process. Practically, this ends up meaning you can only
    /// have 1 test block per integration test that uses a
    /// `DummyArkFrontendRprofile`.
    pub fn lock() -> Self {
        Self::init();

        Self {
            inner: DummyArkFrontend::lock(),
        }
    }

    /// Initialize with user level `.Rprofile` enabled
    fn init() {
        let mut options = DummyArkFrontendOptions::default();
        options.user_r_profile = true;
        let status = FRONTEND.set(Arc::new(Mutex::new(DummyArkFrontend::init(options))));

        if status.is_err() {
            panic!("You can only call `DummyArkFrontendRprofile::lock()` once per process.");
        }

        FRONTEND.get().unwrap();
    }
}

// Allow method calls to be forwarded to inner type
impl Deref for DummyArkFrontendRprofile {
    type Target = DummyFrontend;

    fn deref(&self) -> &Self::Target {
        Deref::deref(&self.inner)
    }
}

impl DerefMut for DummyArkFrontendRprofile {
    fn deref_mut(&mut self) -> &mut Self::Target {
        DerefMut::deref_mut(&mut self.inner)
    }
}

impl Default for DummyArkFrontendOptions {
    fn default() -> Self {
        Self {
            interactive: true,
            site_r_profile: false,
            user_r_profile: false,
            r_environ: false,
            session_mode: SessionMode::Console,
            default_repos: DefaultRepos::Auto,
            startup_file: None,
        }
    }
}
