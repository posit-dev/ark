//
// tracing.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
// Tracing infrastructure for observing DAP and kernel messages during tests.
// Enable tracing by setting the ARK_TEST_TRACE environment variable:
//
//   ARK_TEST_TRACE=1 just test test_name
//
// Or for more selective tracing:
//   ARK_TEST_TRACE=dap       # Only DAP events
//   ARK_TEST_TRACE=iopub     # Only IOPub messages
//   ARK_TEST_TRACE=all       # Both (same as ARK_TEST_TRACE=1)
//

use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::time::Instant;

use amalthea::wire::jupyter_message::Message;
use amalthea::wire::status::ExecutionState;
use amalthea::wire::stream::Stream;
use dap::events::Event;

/// Global start time for relative timestamps
static START_TIME: OnceLock<Instant> = OnceLock::new();

/// Sequence counter for ordering messages across channels
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Whether DAP tracing is enabled
static DAP_ENABLED: AtomicBool = AtomicBool::new(false);

/// Whether IOPub tracing is enabled
static IOPUB_ENABLED: AtomicBool = AtomicBool::new(false);

/// Initialize tracing based on environment variable.
/// Called automatically on first trace call.
fn init_tracing() {
    START_TIME.get_or_init(|| {
        let trace_var = std::env::var("ARK_TEST_TRACE").unwrap_or_default();
        let trace_var = trace_var.to_lowercase();

        let (dap, iopub) = match trace_var.as_str() {
            "1" | "all" | "true" => (true, true),
            "dap" => (true, false),
            "iopub" | "kernel" => (false, true),
            _ => (false, false),
        };

        DAP_ENABLED.store(dap, Ordering::Relaxed);
        IOPUB_ENABLED.store(iopub, Ordering::Relaxed);

        if dap || iopub {
            eprintln!(
                "Tracing: DAP={}, IOPub={}",
                if dap { "on" } else { "off" },
                if iopub { "on" } else { "off" }
            );
        }

        Instant::now()
    });
}

/// Check if DAP tracing is enabled
pub fn is_dap_tracing_enabled() -> bool {
    init_tracing();
    DAP_ENABLED.load(Ordering::Relaxed)
}

/// Check if IOPub tracing is enabled
pub fn is_iopub_tracing_enabled() -> bool {
    init_tracing();
    IOPUB_ENABLED.load(Ordering::Relaxed)
}

/// Get relative timestamp in milliseconds
fn timestamp_ms() -> u64 {
    init_tracing();
    START_TIME.get().unwrap().elapsed().as_millis() as u64
}

/// Get next sequence number
fn next_seq() -> u64 {
    SEQUENCE.fetch_add(1, Ordering::Relaxed)
}

/// Format a DAP event for display
fn format_dap_event(event: &Event) -> String {
    match event {
        Event::Stopped(body) => {
            let reason = format!("{:?}", body.reason);
            let focus = body.preserve_focus_hint.unwrap_or(false);
            format!("Stopped(reason={}, preserve_focus={})", reason, focus)
        },
        Event::Continued(body) => {
            let all = body.all_threads_continued.unwrap_or(false);
            format!("Continued(all_threads={})", all)
        },
        Event::Breakpoint(body) => {
            let bp = &body.breakpoint;
            let verified = bp.verified;
            let line = bp.line.map(|l| l.to_string()).unwrap_or_else(|| "?".into());
            let id = bp.id.map(|i| i.to_string()).unwrap_or_else(|| "?".into());
            format!(
                "Breakpoint(id={}, line={}, verified={})",
                id, line, verified
            )
        },
        Event::Invalidated(body) => {
            let areas = body
                .areas
                .as_ref()
                .map(|a| format!("{:?}", a))
                .unwrap_or_else(|| "all".into());
            format!("Invalidated(areas={})", areas)
        },
        Event::Terminated(_) => "Terminated".to_string(),
        Event::Exited(body) => format!("Exited(code={})", body.exit_code),
        Event::Thread(body) => {
            format!("Thread(id={}, reason={:?})", body.thread_id, body.reason)
        },
        Event::Output(body) => {
            let cat = body
                .category
                .as_ref()
                .map(|c| format!("{:?}", c))
                .unwrap_or_else(|| "?".into());
            let output = &body.output;
            let truncated = if output.len() > 50 {
                format!("{}...", &output[..47])
            } else {
                output.clone()
            };
            format!("Output(cat={}, {:?})", cat, truncated)
        },
        Event::Initialized => "Initialized".to_string(),
        _ => format!("{:?}", event),
    }
}

/// Trace a DAP event being received
pub fn trace_dap_event(event: &Event) {
    if !is_dap_tracing_enabled() {
        return;
    }

    let seq = next_seq();
    let ts = timestamp_ms();
    let formatted = format_dap_event(event);

    eprintln!("│ {:>6}ms │ #{:<4} │ DAP    │ ← {}", ts, seq, formatted);
}

/// Trace a DAP request being sent
pub fn trace_dap_request(command: &str) {
    if !is_dap_tracing_enabled() {
        return;
    }

    let seq = next_seq();
    let ts = timestamp_ms();

    eprintln!(
        "│ {:>6}ms │ #{:<4} │ DAP    │ → Request({})",
        ts, seq, command
    );
}

/// Trace a DAP response being received
pub fn trace_dap_response(command: &str, success: bool) {
    if !is_dap_tracing_enabled() {
        return;
    }

    let seq = next_seq();
    let ts = timestamp_ms();
    let status = if success { "ok" } else { "err" };

    eprintln!(
        "│ {:>6}ms │ #{:<4} │ DAP    │ ← Response({}, {})",
        ts, seq, command, status
    );
}

/// IOPub message types for tracing
#[derive(Debug, Clone)]
pub enum IoPubTrace {
    Busy,
    Idle,
    ExecuteInput { code: String },
    ExecuteResult,
    ExecuteError { message: String },
    Stream { name: String, text: String },
    CommOpen { target: String },
    CommMsg { method: String },
    CommClose,
    Status { state: String },
    Other { msg_type: String },
}

impl IoPubTrace {
    /// Create from message type and optional details
    pub fn from_msg_type(msg_type: &str) -> Self {
        match msg_type {
            "status" => IoPubTrace::Status {
                state: "?".to_string(),
            },
            "execute_input" => IoPubTrace::ExecuteInput {
                code: "...".to_string(),
            },
            "execute_result" => IoPubTrace::ExecuteResult,
            "execute_error" => IoPubTrace::ExecuteError {
                message: "...".to_string(),
            },
            "stream" => IoPubTrace::Stream {
                name: "?".to_string(),
                text: "...".to_string(),
            },
            "comm_open" => IoPubTrace::CommOpen {
                target: "?".to_string(),
            },
            "comm_msg" => IoPubTrace::CommMsg {
                method: "?".to_string(),
            },
            "comm_close" => IoPubTrace::CommClose,
            _ => IoPubTrace::Other {
                msg_type: msg_type.to_string(),
            },
        }
    }
}

/// Format an IOPub trace for display
fn format_iopub_trace(trace: &IoPubTrace) -> String {
    match trace {
        IoPubTrace::Busy => "status(busy)".to_string(),
        IoPubTrace::Idle => "status(idle)".to_string(),
        IoPubTrace::ExecuteInput { code } => {
            let truncated = if code.len() > 30 {
                format!("{}...", &code[..27])
            } else {
                code.clone()
            };
            format!("execute_input({:?})", truncated)
        },
        IoPubTrace::ExecuteResult => "execute_result".to_string(),
        IoPubTrace::ExecuteError { message } => {
            let truncated = if message.len() > 30 {
                format!("{}...", &message[..27])
            } else {
                message.clone()
            };
            format!("execute_error({:?})", truncated)
        },
        IoPubTrace::Stream { name, text } => {
            let truncated = if text.len() > 30 {
                format!("{}...", &text[..27])
            } else {
                text.clone()
            };
            // Replace newlines for display
            let truncated = truncated.replace('\n', "\\n");
            format!("stream({}, {:?})", name, truncated)
        },
        IoPubTrace::CommOpen { target } => format!("comm_open({})", target),
        IoPubTrace::CommMsg { method } => format!("comm_msg({})", method),
        IoPubTrace::CommClose => "comm_close".to_string(),
        IoPubTrace::Status { state } => format!("status({})", state),
        IoPubTrace::Other { msg_type } => format!("{}(?)", msg_type),
    }
}

/// Trace an IOPub message being received
pub fn trace_iopub_message(trace: &IoPubTrace) {
    if !is_iopub_tracing_enabled() {
        return;
    }

    let seq = next_seq();
    let ts = timestamp_ms();
    let formatted = format_iopub_trace(trace);

    eprintln!("│ {:>6}ms │ #{:<4} │ IOPub  │ ← {}", ts, seq, formatted);
}

/// Trace an IOPub `Message` directly.
///
/// Converts the message to an `IoPubTrace` and traces it.
pub fn trace_iopub_msg(msg: &Message) {
    if !is_iopub_tracing_enabled() {
        return;
    }

    let trace = match msg {
        Message::Status(status) => match status.content.execution_state {
            ExecutionState::Busy => IoPubTrace::Busy,
            ExecutionState::Idle => IoPubTrace::Idle,
            ExecutionState::Starting => IoPubTrace::Status {
                state: "starting".to_string(),
            },
        },
        Message::ExecuteInput(input) => IoPubTrace::ExecuteInput {
            code: input.content.code.clone(),
        },
        Message::ExecuteResult(_) => IoPubTrace::ExecuteResult,
        Message::ExecuteError(err) => IoPubTrace::ExecuteError {
            message: err.content.exception.evalue.clone(),
        },
        Message::Stream(stream) => {
            let name = match stream.content.name {
                Stream::Stdout => "stdout",
                Stream::Stderr => "stderr",
            };
            IoPubTrace::Stream {
                name: name.to_string(),
                text: stream.content.text.clone(),
            }
        },
        Message::CommOpen(comm) => IoPubTrace::CommOpen {
            target: comm.content.target_name.clone(),
        },
        Message::CommMsg(comm) => {
            let method = comm
                .content
                .data
                .get("method")
                .and_then(|m| m.as_str())
                .unwrap_or("?")
                .to_string();
            IoPubTrace::CommMsg { method }
        },
        Message::CommClose(_) => IoPubTrace::CommClose,
        _ => IoPubTrace::Other {
            msg_type: format!("{:?}", std::mem::discriminant(msg)),
        },
    };

    trace_iopub_message(&trace);
}

/// Trace a shell request being sent
pub fn trace_shell_request(msg_type: &str, code: Option<&str>) {
    if !is_iopub_tracing_enabled() {
        return;
    }

    let seq = next_seq();
    let ts = timestamp_ms();

    let detail = if let Some(c) = code {
        let truncated = if c.len() > 30 {
            format!("{}...", &c[..27])
        } else {
            c.to_string()
        };
        format!("({:?})", truncated)
    } else {
        String::new()
    };

    eprintln!(
        "│ {:>6}ms │ #{:<4} │ Shell  │ → {}{}",
        ts, seq, msg_type, detail
    );
}

/// Trace a shell reply being received
pub fn trace_shell_reply(msg_type: &str, status: &str) {
    if !is_iopub_tracing_enabled() {
        return;
    }

    let seq = next_seq();
    let ts = timestamp_ms();

    eprintln!(
        "│ {:>6}ms │ #{:<4} │ Shell  │ ← {}({})",
        ts, seq, msg_type, status
    );
}

/// Print a separator line in the trace output
pub fn trace_separator(label: &str) {
    if !is_dap_tracing_enabled() && !is_iopub_tracing_enabled() {
        return;
    }

    let ts = timestamp_ms();
    eprintln!("├──{:>4}ms──┼───────┼────────┼─ {} ─", ts, label);
}

/// Print a note in the trace output
pub fn trace_note(note: &str) {
    if !is_dap_tracing_enabled() && !is_iopub_tracing_enabled() {
        return;
    }

    let ts = timestamp_ms();
    eprintln!("│ {:>6}ms │       │  NOTE  │ {}", ts, note);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_dap_stopped() {
        use dap::events::StoppedEventBody;
        use dap::types::StoppedEventReason;

        let event = Event::Stopped(StoppedEventBody {
            reason: StoppedEventReason::Step,
            description: None,
            thread_id: Some(-1),
            preserve_focus_hint: Some(false),
            text: None,
            all_threads_stopped: Some(true),
            hit_breakpoint_ids: None,
        });

        let formatted = format_dap_event(&event);
        assert!(formatted.contains("Stopped"));
        assert!(formatted.contains("Step"));
    }

    #[test]
    fn test_format_iopub_stream() {
        let trace = IoPubTrace::Stream {
            name: "stdout".to_string(),
            text: "Hello, world!\nLine 2".to_string(),
        };

        let formatted = format_iopub_trace(&trace);
        assert!(formatted.contains("stream"));
        assert!(formatted.contains("stdout"));
        assert!(formatted.contains("\\n")); // Newline escaped
    }
}
