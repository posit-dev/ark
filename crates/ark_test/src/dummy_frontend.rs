use std::io::Seek;
use std::io::Write;
use std::ops::Deref;
use std::ops::DerefMut;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;

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

use crate::tracing::trace_iopub_msg;
use crate::tracing::trace_separator;
use crate::tracing::trace_shell_reply;
use crate::tracing::trace_shell_request;
use crate::DapClient;

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

    /// Receive from IOPub, skipping any Stream messages, and assert Busy status.
    ///
    /// Use this when late-arriving Stream messages from previous operations
    /// can interleave with the expected Busy message.
    #[track_caller]
    pub fn recv_iopub_busy_skip_streams(&self) {
        loop {
            let msg = self.recv_iopub();
            trace_iopub_msg(&msg);
            match msg {
                Message::Stream(_) => continue,
                Message::Status(data) => {
                    assert_eq!(
                        data.content.execution_state,
                        amalthea::wire::status::ExecutionState::Busy,
                        "Expected Busy status"
                    );
                    return;
                },
                other => panic!("Expected Busy status, got {:?}", other),
            }
        }
    }

    /// Receive from IOPub, skipping any Stream messages, and assert ExecuteInput.
    ///
    /// Use this when late-arriving Stream messages from previous operations
    /// can interleave with the expected ExecuteInput message.
    #[track_caller]
    pub fn recv_iopub_execute_input_skip_streams(&self) {
        loop {
            let msg = self.recv_iopub();
            trace_iopub_msg(&msg);
            match msg {
                Message::Stream(_) => continue,
                Message::ExecuteInput(_) => return,
                other => panic!("Expected ExecuteInput, got {:?}", other),
            }
        }
    }

    /// Receive from IOPub, skipping any Stream messages, and assert Idle status.
    ///
    /// Use this when late-arriving Stream messages from previous operations
    /// can interleave with the expected Idle message.
    #[track_caller]
    pub fn recv_iopub_idle_skip_streams(&self) {
        loop {
            let msg = self.recv_iopub();
            trace_iopub_msg(&msg);
            match msg {
                Message::Stream(_) => continue,
                Message::Status(data) => {
                    assert_eq!(
                        data.content.execution_state,
                        amalthea::wire::status::ExecutionState::Idle,
                        "Expected Idle status"
                    );
                    return;
                },
                other => panic!("Expected Idle status, got {:?}", other),
            }
        }
    }

    /// Receive from IOPub and assert a `start_debug` comm message.
    #[track_caller]
    pub fn recv_iopub_start_debug(&self) {
        let msg = self.recv_iopub();
        trace_iopub_msg(&msg);
        match msg {
            Message::CommMsg(data) => {
                let method = data.content.data.get("method").and_then(|v| v.as_str());
                assert_eq!(
                    method,
                    Some("start_debug"),
                    "Expected start_debug comm message"
                );
            },
            other => panic!("Expected CommMsg with start_debug, got {:?}", other),
        }
    }

    /// Receive from IOPub, skipping any Stream messages, and assert a `start_debug` comm message.
    ///
    /// Use this when late-arriving Stream messages from previous operations
    /// can interleave with the expected start_debug message.
    #[track_caller]
    pub fn recv_iopub_start_debug_skip_streams(&self) {
        loop {
            let msg = self.recv_iopub();
            trace_iopub_msg(&msg);
            match msg {
                Message::Stream(_) => continue,
                Message::CommMsg(data) => {
                    let method = data.content.data.get("method").and_then(|v| v.as_str());
                    assert_eq!(
                        method,
                        Some("start_debug"),
                        "Expected start_debug comm message"
                    );
                    return;
                },
                other => panic!("Expected CommMsg with start_debug, got {:?}", other),
            }
        }
    }

    /// Receive from IOPub and assert a `stop_debug` comm message.
    #[track_caller]
    pub fn recv_iopub_stop_debug(&self) {
        let msg = self.recv_iopub();
        trace_iopub_msg(&msg);
        match msg {
            Message::CommMsg(data) => {
                let method = data.content.data.get("method").and_then(|v| v.as_str());
                assert_eq!(
                    method,
                    Some("stop_debug"),
                    "Expected stop_debug comm message"
                );
            },
            other => panic!("Expected CommMsg with stop_debug, got {:?}", other),
        }
    }

    /// Receive from IOPub, skipping any Stream messages, and assert a `stop_debug` comm message.
    ///
    /// Use this when late-arriving Stream messages from previous operations
    /// can interleave with the expected stop_debug message.
    #[track_caller]
    pub fn recv_iopub_stop_debug_skip_streams(&self) {
        loop {
            let msg = self.recv_iopub();
            trace_iopub_msg(&msg);
            match msg {
                Message::Stream(_) => continue,
                Message::CommMsg(data) => {
                    let method = data.content.data.get("method").and_then(|v| v.as_str());
                    assert_eq!(
                        method,
                        Some("stop_debug"),
                        "Expected stop_debug comm message"
                    );
                    return;
                },
                other => panic!("Expected CommMsg with stop_debug, got {:?}", other),
            }
        }
    }

    /// Receive stream messages until accumulated content contains the expected text.
    ///
    /// This handles stream fragmentation by accumulating output until the expected
    /// substring is found. Panics if a non-stream message is received.
    #[track_caller]
    pub fn recv_iopub_stream_stdout_containing(&self, expected: &str) {
        use amalthea::wire::stream::Stream;

        let mut accumulated = String::new();
        loop {
            let msg = self.recv_iopub();
            trace_iopub_msg(&msg);
            match msg {
                Message::Stream(data) => {
                    assert_eq!(
                        data.content.name,
                        Stream::Stdout,
                        "Expected stdout stream, got {:?}",
                        data.content.name
                    );
                    accumulated.push_str(&data.content.text);
                    if accumulated.contains(expected) {
                        return;
                    }
                },
                other => panic!(
                    "Expected Stream message containing {:?}, got {:?}",
                    expected, other
                ),
            }
        }
    }

    /// Receive stream messages until accumulated content contains the expected text (stderr).
    ///
    /// This handles stream fragmentation by accumulating output until the expected
    /// substring is found. Panics if a non-stream message is received.
    #[track_caller]
    pub fn recv_iopub_stream_stderr_containing(&self, expected: &str) {
        use amalthea::wire::stream::Stream;

        let mut accumulated = String::new();
        loop {
            let msg = self.recv_iopub();
            trace_iopub_msg(&msg);
            match msg {
                Message::Stream(data) => {
                    assert_eq!(
                        data.content.name,
                        Stream::Stderr,
                        "Expected stderr stream, got {:?}",
                        data.content.name
                    );
                    accumulated.push_str(&data.content.text);
                    if accumulated.contains(expected) {
                        return;
                    }
                },
                other => panic!(
                    "Expected Stream message containing {:?}, got {:?}",
                    expected, other
                ),
            }
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
        trace_separator(&format!("source_and_hit_bp({}) START", file.filename));
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
        trace_separator(&format!("source_and_hit_bp({}) END", file.filename));
    }

    /// Receive IOPub messages for a breakpoint hit with auto-stepping.
    ///
    /// The message sequence is (in order):
    /// 1. stream ("Called from:") - R's initial debug output
    /// 2. start_debug (entering .ark_breakpoint wrapper)
    /// 3. stop_debug (auto-stepping out of wrapper)
    /// 4. stream ("debug at") - R's debug output at user expression
    /// 5. start_debug (at user expression)
    /// 6. idle
    #[track_caller]
    pub fn recv_iopub_breakpoint_hit(&self) {
        // Use _skip_streams variants because stream messages can be split across
        // multiple fragments, and streams may interleave with debug comm messages.
        self.recv_iopub_stream_stdout_containing("Called from:");
        self.recv_iopub_start_debug_skip_streams();
        self.recv_iopub_stop_debug_skip_streams();
        self.recv_iopub_stream_stdout_containing("debug at");
        self.recv_iopub_start_debug_skip_streams();
        self.recv_iopub_idle_skip_streams();
    }

    /// Receive IOPub messages for a breakpoint hit from a direct function call.
    ///
    /// This is similar to `recv_iopub_breakpoint_hit` but for direct function calls
    /// (e.g., `foo()`) rather than `source()`. When the function has source references
    /// (from a file created with `SourceFile::new()`), R will print `"debug at"`.
    ///
    /// The message sequence is (in order):
    /// 1. stream ("Called from:") - R's initial debug output
    /// 2. start_debug (entering .ark_breakpoint wrapper)
    /// 3. stop_debug (auto-stepping out of wrapper)
    /// 4. stream ("debug at") - R's debug output at user expression
    /// 5. start_debug (at user expression)
    /// 6. idle
    #[track_caller]
    pub fn recv_iopub_breakpoint_hit_direct(&self) {
        trace_separator("recv_iopub_breakpoint_hit_direct START");
        // Use _skip_streams variants because stream messages can be split across
        // multiple fragments, and streams may interleave with debug comm messages.
        self.recv_iopub_stream_stdout_containing("Called from:");
        self.recv_iopub_start_debug_skip_streams();
        self.recv_iopub_stop_debug_skip_streams();
        self.recv_iopub_stream_stdout_containing("debug at");
        self.recv_iopub_start_debug_skip_streams();
        self.recv_iopub_idle_skip_streams();
        trace_separator("recv_iopub_breakpoint_hit_direct END");
    }

    /// Source a file that was created with `SourceFile::new()`.
    ///
    /// The code must contain `browser()` or a breakpoint to enter debug mode.
    /// The caller must still receive the DAP `Stopped` event.
    ///
    /// The message sequence is (in order):
    /// 1. stream ("Called from:") - R's debug output
    /// 2. start_debug (entering debug mode)
    /// 3. idle
    #[track_caller]
    pub fn source_debug_file(&self, file: &SourceFile) {
        trace_separator(&format!("source_debug({})", file.filename));
        self.send_execute_request(
            &format!("source('{}')", file.path),
            ExecuteRequestOptions::default(),
        );
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        // Use _skip_streams variant because stream messages can be split across
        // multiple fragments, and streams may interleave with debug comm messages.
        self.recv_iopub_stream_stdout_containing("Called from:");
        self.recv_iopub_start_debug_skip_streams();
        self.recv_iopub_idle_skip_streams();

        self.recv_shell_execute_reply();
    }

    /// Source a file containing code that enters debug mode (e.g., via `browser()`).
    ///
    /// Returns a `SourceFile` containing the temp file (which must be kept alive)
    /// and the filename for use in assertions.
    ///
    /// The caller must still receive the DAP `Stopped` event.
    #[track_caller]
    pub fn send_source(&self, code: &str) -> SourceFile {
        let line_count = code.lines().count() as u32;
        let mut file = NamedTempFile::new().unwrap();
        write!(file, "{code}").unwrap();

        // Use forward slashes for R compatibility on Windows (backslashes would be
        // interpreted as escape sequences in R strings)
        let path = file.path().to_string_lossy().replace('\\', "/");
        let url = ExtUrl::from_file_path(file.path()).unwrap();
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

        // Message sequence (in order):
        // 1. stream ("Called from:") - R's debug output
        // 2. start_debug (entering debug mode)
        // 3. idle
        // Use _skip_streams variant because stream messages can be split across
        // multiple fragments, and the stream containing "Called from:" may be
        // followed by additional stream fragments.
        self.recv_iopub_stream_stdout_containing("Called from:");
        self.recv_iopub_start_debug_skip_streams();
        self.recv_iopub_idle_skip_streams();

        self.recv_shell_execute_reply();

        SourceFile {
            file,
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
        self.recv_iopub_start_debug();
        self.recv_iopub_execute_result();
        self.recv_iopub_idle();
        self.recv_shell_execute_reply()
    }

    /// Execute `Q` to quit the browser and receive all expected messages.
    #[track_caller]
    pub fn debug_send_quit(&self) -> u32 {
        trace_separator("debug_send_quit START");
        self.send_execute_request("Q", ExecuteRequestOptions::default());
        // Use stream-skipping variants because late-arriving debug output
        // from previous operations can interleave here.
        self.recv_iopub_busy_skip_streams();
        self.recv_iopub_execute_input_skip_streams();
        self.recv_iopub_stop_debug_skip_streams();
        self.recv_iopub_idle_skip_streams();
        let result = self.recv_shell_execute_reply();
        trace_separator("debug_send_quit END");
        result
    }

    /// Execute `c` (continue) to next browser() breakpoint in a sourced file.
    ///
    /// When continuing from one browser() to another, R outputs "Called from:"
    /// instead of "debug at", so this needs a different message pattern.
    ///
    /// The message sequence is (in order):
    /// 1. stop_debug (leaving current location)
    /// 2. stream ("Called from:") - R's debug output
    /// 3. start_debug (at new breakpoint)
    /// 4. idle
    #[track_caller]
    pub fn debug_send_continue_to_breakpoint(&self) -> u32 {
        self.send_execute_request("c", ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        // Use _skip_streams variants because stream messages can be split across
        // multiple fragments, and streams may interleave with debug comm messages.
        self.recv_iopub_stop_debug_skip_streams();
        self.recv_iopub_stream_stdout_containing("Called from:");
        self.recv_iopub_start_debug_skip_streams();
        self.recv_iopub_idle_skip_streams();

        self.recv_shell_execute_reply()
    }

    /// Execute an expression while in debug mode and receive all expected messages.
    ///
    /// This is for evaluating expressions that don't advance the debugger (e.g., `1`, `x`).
    /// The caller must still receive the DAP `Stopped` event with `preserve_focus_hint=true`.
    ///
    /// The message sequence is (in order):
    /// 1. stop_debug (leaving current location)
    /// 2. start_debug (back at same location)
    /// 3. execute_result (the evaluated expression result)
    /// 4. idle
    #[track_caller]
    pub fn debug_send_expr(&self, expr: &str) -> u32 {
        self.send_execute_request(expr, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        // Use _skip_streams variants because stream messages can be split across
        // multiple fragments, and streams may interleave with debug comm messages.
        self.recv_iopub_stop_debug_skip_streams();
        self.recv_iopub_start_debug_skip_streams();
        self.recv_iopub_execute_result();
        self.recv_iopub_idle_skip_streams();

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
    ///
    /// The message sequence is (in order):
    /// 1. stream (stderr with error message)
    /// 2. stop_debug (leaving current location)
    /// 3. start_debug (back at same location)
    /// 4. idle
    #[track_caller]
    pub fn debug_send_error_expr(&self, expr: &str) -> u32 {
        self.send_execute_request(expr, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        // Use _skip_streams variants because stream messages can be split across
        // multiple fragments, and streams may interleave with debug comm messages.
        self.recv_iopub_stream_stderr_containing("Error");
        self.recv_iopub_stop_debug_skip_streams();
        self.recv_iopub_start_debug_skip_streams();
        self.recv_iopub_idle_skip_streams();

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
    ///
    /// The message sequence is (in order):
    /// 1. stop_debug (leaving current location)
    /// 2. stream ("debug at") - R's debug output
    /// 3. start_debug (at new location)
    /// 4. idle
    #[track_caller]
    pub fn debug_send_step_command(&self, cmd: &str) -> u32 {
        trace_separator(&format!("debug_step({})", cmd));
        self.send_execute_request(cmd, ExecuteRequestOptions::default());
        self.recv_iopub_busy();
        self.recv_iopub_execute_input();

        // Use _skip_streams variants because stream messages can be split across
        // multiple fragments, and streams may interleave with debug comm messages.
        self.recv_iopub_stop_debug_skip_streams();
        self.recv_iopub_stream_stdout_containing("debug at");
        self.recv_iopub_start_debug_skip_streams();
        self.recv_iopub_idle_skip_streams();

        self.recv_shell_execute_reply()
    }
}

/// Result of sourcing a file via `send_source()`.
///
/// The temp file is kept alive as long as this struct exists.
pub struct SourceFile {
    file: NamedTempFile,
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

        // Use forward slashes for R compatibility on Windows (backslashes would be
        // interpreted as escape sequences in R strings)
        let path = file.path().to_string_lossy().replace('\\', "/");
        let url = ExtUrl::from_file_path(file.path()).unwrap();
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
            file,
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

    /// Rewrite the file with new content.
    ///
    /// Use this for tests that need to modify the file after creation
    /// (e.g., testing hash change detection).
    pub fn rewrite(&mut self, code: &str) {
        self.file.rewind().unwrap();
        self.file.as_file_mut().set_len(0).unwrap();

        write!(self.file, "{code}").unwrap();

        self.file.flush().unwrap();
        self.line_count = code.lines().count() as u32;
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

// Check that we haven't left crumbs behind.
//
// Certain messages are allowed to remain because they can arrive asynchronously:
// - Stream messages: can interleave with other operations due to batching.
// - CommMsg with method "execute": `DapClient::drop()` calls `disconnect()` which
//   sends a Disconnect request. If ark is still debugging, `handle_disconnect()`
//   sends an `execute Q` comm message to quit the browser. Since `DapClient` is
//   dropped before `DummyArkFrontend` (reverse declaration order), this message
//   can arrive here after the test has otherwise completed cleanly.
impl Drop for DummyArkFrontend {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }

        // Drain any pending IOPub messages
        let mut unexpected_messages: Vec<Message> = Vec::new();
        while self.iopub_socket.has_incoming_data().unwrap() {
            let msg = Message::read_from_socket(&self.iopub_socket).unwrap();

            let exempt = match &msg {
                Message::Stream(_) => true,
                Message::CommMsg(comm) => {
                    comm.content.data.get("method").and_then(|v| v.as_str()) == Some("execute") &&
                        comm.content
                            .data
                            .get("params")
                            .and_then(|p| p.get("command"))
                            .and_then(|c| c.as_str()) ==
                            Some("Q")
                },
                _ => false,
            };

            if !exempt {
                unexpected_messages.push(msg);
            }
        }

        // Fail if any unexpected IOPub messages were left behind
        if !unexpected_messages.is_empty() {
            panic!(
                "IOPub socket has {} unexpected message(s) on exit:\n{:#?}",
                unexpected_messages.len(),
                unexpected_messages
            );
        }

        // Check other sockets strictly (no leniency for non-IOPub)
        let mut shell_messages: Vec<Message> = Vec::new();
        let mut stdin_messages: Vec<Message> = Vec::new();

        while self.shell_socket.has_incoming_data().unwrap() {
            if let Ok(msg) = Message::read_from_socket(&self.shell_socket) {
                shell_messages.push(msg);
            }
        }
        while self.stdin_socket.has_incoming_data().unwrap() {
            if let Ok(msg) = Message::read_from_socket(&self.stdin_socket) {
                stdin_messages.push(msg);
            }
        }

        if !shell_messages.is_empty() || !stdin_messages.is_empty() {
            panic!(
                "Non-IOPub sockets have unexpected messages on exit:\n\
                 Shell: {:#?}\n\
                 StdIn: {:#?}",
                shell_messages, stdin_messages
            );
        }
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
