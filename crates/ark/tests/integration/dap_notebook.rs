//
// dap_notebook.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use amalthea::wire::jupyter_message::Message;
use ark_test::DummyArkFrontendNotebook;
use ark_test::IopubExpectation;

fn find_debug_event<'a>(msgs: &'a [Message], event: &str) -> &'a serde_json::Value {
    msgs.iter()
        .find_map(|m| match m {
            Message::DebugEvent(data) if data.content.content["event"] == event => {
                Some(&data.content.content)
            },
            _ => None,
        })
        .unwrap_or_else(|| panic!("No DebugEvent with event={event:?} found"))
}

/// Send an `evaluate` `debug_request` and return the reply.
///
/// `evaluate` runs through `try_r_task()`, which runs it only if R can serve it.
/// This re-sends until R can, for waits that nothing announces: cold start,
/// before R has reached its first prompt, and a callback returning to the
/// top-level event loop. Use `notebook_evaluate_once()` when the request
/// follows a readiness announcement and must land on the first attempt. Busy
/// replies come back immediately, so we pace the attempts with a short sleep.
/// The control thread brackets each attempt with busy/idle on IOPub, and the
/// R-thread evaluation emits nothing there (printed output is captured), so
/// each is the same busy/reply/idle as any other control-only request.
fn notebook_evaluate(
    frontend: &DummyArkFrontendNotebook,
    seq: i64,
    expression: &str,
) -> serde_json::Value {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        frontend.send_debug_request(serde_json::json!({
            "type": "request",
            "seq": seq,
            "command": "evaluate",
            "arguments": { "expression": expression }
        }));
        frontend.recv_iopub_busy();
        let reply = frontend.recv_debug_reply();
        frontend.recv_iopub_idle();

        let busy = reply["success"] == false && reply["message"] == "R is busy";
        if !busy {
            return reply;
        }
        if std::time::Instant::now() >= deadline {
            panic!("`evaluate` kept returning \"R is busy\"; R never reached a prompt");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Send a single `evaluate` `debug_request` in the global environment and
/// return the reply, along with how long the reply took to arrive.
fn notebook_evaluate_once(
    frontend: &DummyArkFrontendNotebook,
    seq: i64,
    expression: &str,
) -> (serde_json::Value, std::time::Duration) {
    let start = std::time::Instant::now();
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": seq,
        "command": "evaluate",
        "arguments": { "expression": expression }
    }));
    let reply = frontend.recv_debug_reply();
    (reply, start.elapsed())
}

#[test]
fn test_notebook_debug_info() {
    let frontend = DummyArkFrontendNotebook::lock();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "debugInfo",
        "arguments": {}
    }));
    frontend.recv_iopub_busy();
    let reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    assert_eq!(reply["success"], true);
    assert_eq!(reply["body"]["isStarted"], true);
    assert_eq!(reply["body"]["hashMethod"], "Murmur2");
    assert_eq!(reply["body"]["hashSeed"], 0);
    let prefix = reply["body"]["tmpFilePrefix"].as_str().unwrap();
    assert!(prefix.contains("ark-debug-"));
    assert_eq!(reply["body"]["tmpFileSuffix"], ".r");
}

#[test]
fn test_notebook_dump_cell() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "x <- 1\nprint(x)";

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": code }
    }));
    frontend.recv_iopub_busy();
    let reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    assert_eq!(reply["success"], true);
    let source_path = reply["body"]["sourcePath"].as_str().unwrap();
    assert!(source_path.contains("ark-debug-"));
    assert!(source_path.ends_with(".r"));

    // File should actually exist on disk with the cell contents
    assert!(std::path::Path::new(source_path).exists());
    assert_eq!(std::fs::read_to_string(source_path).unwrap(), code);
}

#[test]
fn test_notebook_dump_cell_deterministic() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "x <- 42\ny <- x + 1";

    // Dump the same cell code twice
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": code }
    }));
    frontend.recv_iopub_busy();
    let reply1 = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "dumpCell",
        "arguments": { "code": code }
    }));
    frontend.recv_iopub_busy();
    let reply2 = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Same code should produce the same source path (Murmur2 hash)
    assert_eq!(
        reply1["body"]["sourcePath"].as_str().unwrap(),
        reply2["body"]["sourcePath"].as_str().unwrap()
    );
}

#[test]
fn test_notebook_dump_cell_different_code() {
    let frontend = DummyArkFrontendNotebook::lock();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": "cell_a" }
    }));
    frontend.recv_iopub_busy();
    let reply1 = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "dumpCell",
        "arguments": { "code": "cell_b" }
    }));
    frontend.recv_iopub_busy();
    let reply2 = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Different code should produce different paths
    assert_ne!(
        reply1["body"]["sourcePath"].as_str().unwrap(),
        reply2["body"]["sourcePath"].as_str().unwrap()
    );
}

#[test]
fn test_notebook_configuration_done() {
    let frontend = DummyArkFrontendNotebook::lock();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "configurationDone",
        "arguments": {}
    }));
    frontend.recv_iopub_busy();
    let reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    assert_eq!(reply["success"], true);
    assert_eq!(reply["command"], "configurationDone");
}

#[test]
fn test_notebook_dump_cell_then_set_breakpoints() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "x <- 1\ny <- 2\nz <- x + y";

    // Dump the cell to a temp file
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": code }
    }));
    frontend.recv_iopub_busy();
    let dump_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    let source_path = dump_reply["body"]["sourcePath"].as_str().unwrap();

    // Set breakpoints on the dumped file
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": source_path },
            "breakpoints": [{ "line": 2 }]
        }
    }));
    frontend.recv_iopub_busy();
    let bp_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    assert_eq!(bp_reply["success"], true);
    let breakpoints = bp_reply["body"]["breakpoints"].as_array().unwrap();
    assert_eq!(breakpoints.len(), 1);
    assert_eq!(breakpoints[0]["line"], 2);
}

#[test]
fn test_notebook_set_multiple_breakpoints() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "a <- 1\nb <- 2\nc <- 3\nd <- 4";

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": code }
    }));
    frontend.recv_iopub_busy();
    let dump_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    let source_path = dump_reply["body"]["sourcePath"].as_str().unwrap();

    // Set breakpoints on lines 2 and 4
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": source_path },
            "breakpoints": [{ "line": 2 }, { "line": 4 }]
        }
    }));
    frontend.recv_iopub_busy();
    let bp_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    assert_eq!(bp_reply["success"], true);
    let breakpoints = bp_reply["body"]["breakpoints"].as_array().unwrap();
    assert_eq!(breakpoints.len(), 2);
    assert_eq!(breakpoints[0]["line"], 2);
    assert_eq!(breakpoints[1]["line"], 4);
}

#[test]
fn test_notebook_clear_breakpoints() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "x <- 1\ny <- 2";

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": code }
    }));
    frontend.recv_iopub_busy();
    let dump_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    let source_path = dump_reply["body"]["sourcePath"].as_str().unwrap();

    // Set a breakpoint
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": source_path },
            "breakpoints": [{ "line": 2 }]
        }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Clear breakpoints by sending an empty list
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 3,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": source_path },
            "breakpoints": []
        }
    }));
    frontend.recv_iopub_busy();
    let bp_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    assert_eq!(bp_reply["success"], true);
    let breakpoints = bp_reply["body"]["breakpoints"].as_array().unwrap();
    assert!(breakpoints.is_empty());
}

#[test]
fn test_notebook_execute_with_cell_id() {
    let frontend = DummyArkFrontendNotebook::lock();

    // Execute a cell with `cellId` in metadata (regression: shouldn't crash)
    frontend.send_execute_request_with_metadata(
        "42",
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "test-cell-1" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 42");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_notebook_execute_multiline_with_cell_id() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "x <- 10\ny <- 20\nx + y";
    frontend.send_execute_request_with_metadata(
        code,
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "test-cell-2" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 30");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_notebook_initialize_via_jupyter_debug() {
    let frontend = DummyArkFrontendNotebook::lock();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "initialize",
        "arguments": {
            "clientID": "test",
            "adapterID": "test",
            "pathFormat": "path",
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "supportsRunInTerminalRequest": false
        }
    }));
    frontend.recv_iopub_busy();

    // `initialize` produces an `Initialized` event on IOPub
    let event = frontend.recv_iopub_debug_event();
    assert_eq!(event["type"], "event");
    assert_eq!(event["event"], "initialized");

    let reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    assert_eq!(reply["success"], true);
    assert_eq!(reply["command"], "initialize");

    // Capabilities should be present
    assert!(reply["body"]["supportsRestartRequest"].as_bool().unwrap());
}

#[test]
fn test_notebook_unknown_dap_command() {
    let frontend = DummyArkFrontendNotebook::lock();

    // Sending a command with invalid structure should get an error response
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "nonexistentCommand",
        "arguments": {}
    }));
    frontend.recv_iopub_busy();
    let reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    assert_eq!(reply["success"], false);
}

#[test]
fn test_notebook_breakpoint_stops_execution() {
    let frontend = DummyArkFrontendNotebook::lock();

    let fn_code = "fn <- function() {\n  x <- 1\n  x <- 2\n  x <- 3\n  x\n}";

    // Dump cell and set a breakpoint at line 3 (x <- 2)
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": fn_code }
    }));
    frontend.recv_iopub_busy();
    let dump_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();
    let source_path = dump_reply["body"]["sourcePath"]
        .as_str()
        .unwrap()
        .to_string();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": &source_path },
            "breakpoints": [{ "line": 3 }]
        }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Attach sets is_connected = true so breakpoints fire
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 3,
        "command": "attach",
        "arguments": { "request": "attach", "type": "notebook" }
    }));
    frontend.recv_iopub_busy();
    // attach produces a Thread started event
    let event = frontend.recv_iopub_debug_event();
    assert_eq!(event["event"], "thread");
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Define the function (breakpoints get injected into the body)
    frontend.send_execute_request_with_metadata(
        fn_code,
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-def" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    // Breakpoint gets verified when the function body is parsed
    let bp_event = frontend.recv_iopub_debug_event();
    assert_eq!(bp_event["event"], "breakpoint");
    assert_eq!(bp_event["body"]["breakpoint"]["verified"], true);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Call the function — should hit breakpoint and kernel stays busy
    frontend.send_execute_request_with_metadata(
        "fn()",
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-call" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Stopped event arrives on IOPub (kernel paused at breakpoint)
    let stopped = frontend.recv_iopub_debug_event();
    assert_eq!(stopped["event"], "stopped");

    // Shell reply hasn't arrived — kernel is still busy
    assert!(!frontend.shell_socket.poll_incoming(200).unwrap());

    // Send "continue" via the debug channel
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 4,
        "command": "continue",
        "arguments": { "threadId": -1 }
    }));
    frontend.recv_debug_reply();

    // Shell reply arrives now — kernel unblocked after continue
    frontend.recv_shell_execute_reply();

    // IOPub messages from the control thread (busy/idle for the debug_request)
    // and the R thread (debug_event Continued, execute_request idle) arrive in
    // unpredictable order since they originate from different threads.
    let msgs = frontend.recv_iopub_interleaved(&[
        // Control thread: debug_request busy/idle
        &[IopubExpectation::BusyControl, IopubExpectation::IdleControl],
        // R thread: continued event, execute result, then execution idle
        &[
            IopubExpectation::DebugEvent,
            IopubExpectation::ExecuteResult,
            IopubExpectation::IdleShell,
        ],
    ]);
    find_debug_event(&msgs, "continued");

    // Disconnect to reset is_connected for other tests
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 5,
        "command": "disconnect",
        "arguments": { "restart": false }
    }));
    frontend.recv_debug_reply();
    // Only the control thread sends IOPub messages here (no R-thread side effects)
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
}

#[test]
#[cfg_attr(target_os = "windows", ignore)]
fn test_notebook_interrupt_at_breakpoint_exits_debugger() {
    let frontend = DummyArkFrontendNotebook::lock();

    let fn_code = "fn3 <- function() {\n  x <- 1\n  x <- 2\n  x <- 3\n  x\n}";

    // Dump cell and set a breakpoint at line 3 (x <- 2)
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": fn_code }
    }));
    frontend.recv_iopub_busy();
    let dump_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();
    let source_path = dump_reply["body"]["sourcePath"]
        .as_str()
        .unwrap()
        .to_string();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": &source_path },
            "breakpoints": [{ "line": 3 }]
        }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Attach so breakpoints fire
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 3,
        "command": "attach",
        "arguments": { "request": "attach", "type": "notebook" }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_iopub_debug_event(); // thread started
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Define the function
    frontend.send_execute_request_with_metadata(
        fn_code,
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-def-int" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let bp_event = frontend.recv_iopub_debug_event();
    assert_eq!(bp_event["event"], "breakpoint");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Call the function — hits breakpoint, kernel stays busy
    frontend.send_execute_request_with_metadata(
        "fn3()",
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-call-int" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let stopped = frontend.recv_iopub_debug_event();
    assert_eq!(stopped["event"], "stopped");

    // Shell reply hasn't arrived — kernel is paused
    assert!(!frontend.shell_socket.poll_incoming(200).unwrap());

    // Send interrupt — in notebook mode this should exit the debugger
    frontend.send_interrupt_request();
    frontend.recv_control_interrupt_reply();

    // Shell reply arrives — kernel unblocked by the Q command
    frontend.recv_shell_execute_reply();

    // IOPub messages from the control thread (interrupt busy/idle) and
    // R thread (debug_event Continued, execute_request idle) race.
    let msgs = frontend.recv_iopub_interleaved(&[
        // Control thread: interrupt_request busy/idle
        &[IopubExpectation::BusyControl, IopubExpectation::IdleControl],
        // R thread: continued event, then execution idle
        &[IopubExpectation::DebugEvent, IopubExpectation::IdleShell],
    ]);
    find_debug_event(&msgs, "continued");

    // Disconnect
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 4,
        "command": "disconnect",
        "arguments": { "restart": false }
    }));
    frontend.recv_debug_reply();
    // Only the control thread sends IOPub messages here (no R-thread side effects)
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
}

#[test]
fn test_notebook_unexpected_browser_routes_to_stdin() {
    let frontend = DummyArkFrontendNotebook::lock();

    // Execute code that calls browser() directly — no debug session active.
    // `browser(); 42` is split into two pending expressions. After quitting
    // the browser, the second expression `42` is evaluated and produces a result.
    frontend.send_execute_request_with_metadata(
        "browser(); 42",
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-browser-stdin" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // The browser prompt is routed to stdin since no debug session is connected
    let prompt = frontend.recv_stdin_input_request();
    assert!(
        prompt.contains("Browse"),
        "Expected Browse prompt, got: {prompt}"
    );

    // User types "Q" to quit the browser
    frontend.send_stdin_input_reply(String::from("Q"));

    // The remaining expression `42` produces a result
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 42");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_notebook_unexpected_browser_continue_via_stdin() {
    let frontend = DummyArkFrontendNotebook::lock();

    // Define a function with browser() inside — no debug session active
    frontend.send_execute_request_with_metadata(
        "fn_stdin <- function() {\n  x <- 1\n  browser()\n  x <- 42\n  x\n}",
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-browser-def" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Call the function — browser() fires, routed to stdin
    frontend.send_execute_request_with_metadata(
        "fn_stdin()",
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-browser-call" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let prompt = frontend.recv_stdin_input_request();
    assert!(
        prompt.contains("Browse"),
        "Expected Browse prompt, got: {prompt}"
    );

    // User types "c" to continue — function runs to completion
    frontend.send_stdin_input_reply(String::from("c"));

    // Function returns 42
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 42");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
#[cfg_attr(target_os = "windows", ignore)]
fn test_notebook_unexpected_browser_interrupt_via_stdin() {
    let frontend = DummyArkFrontendNotebook::lock();

    // Define a function that enters browser() — no debug session active
    frontend.send_execute_request_with_metadata(
        "fn_stdin_int <- function() {\n  browser()\n  42\n}",
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-browser-int-def" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Call the function — browser() fires, routed to stdin
    frontend.send_execute_request_with_metadata(
        "fn_stdin_int()",
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-browser-int-call" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let prompt = frontend.recv_stdin_input_request();
    assert!(
        prompt.contains("Browse"),
        "Expected Browse prompt, got: {prompt}"
    );

    // Shell reply hasn't arrived — kernel is waiting for stdin input
    assert!(!frontend.shell_socket.poll_incoming(200).unwrap());

    // Send interrupt — should exit the browser via Q
    frontend.send_interrupt_request();
    frontend.recv_control_interrupt_reply();

    // IOPub messages from the control thread (interrupt busy/idle) and
    // R thread (execute_request idle) race.
    frontend.recv_iopub_interleaved(&[
        // Control thread: interrupt_request busy/idle
        &[IopubExpectation::BusyControl, IopubExpectation::IdleControl],
        // R thread: execution idle
        &[IopubExpectation::IdleShell],
    ]);

    // Execution completes — the interrupt exited the browser
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_notebook_breakpoints_inert_without_attach() {
    let frontend = DummyArkFrontendNotebook::lock();

    let fn_code = "fn2 <- function() {\n  x <- 1\n  x <- 2\n  x <- 3\n  invisible(x)\n}";

    // Dump cell and set a breakpoint — but do NOT attach
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": fn_code }
    }));
    frontend.recv_iopub_busy();
    let dump_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();
    let source_path = dump_reply["body"]["sourcePath"]
        .as_str()
        .unwrap()
        .to_string();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": &source_path },
            "breakpoints": [{ "line": 3 }]
        }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Define the function (breakpoints are injected but won't fire)
    frontend.send_execute_request_with_metadata(
        fn_code,
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-def-inert" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    // Breakpoint gets verified when the function body is parsed
    let bp_event = frontend.recv_iopub_debug_event();
    assert_eq!(bp_event["event"], "breakpoint");
    assert_eq!(bp_event["body"]["breakpoint"]["verified"], true);
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Call the function — should complete normally (breakpoint is inert)
    frontend.send_execute_request_with_metadata(
        "fn2()",
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-call-inert" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    // No Stopped event — execution completes without stopping
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

#[test]
fn test_notebook_debug_info_reports_breakpoints() {
    let frontend = DummyArkFrontendNotebook::lock();

    let code = "a <- 1\nb <- 2\nc <- 3";

    // Dump a cell
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": code }
    }));
    frontend.recv_iopub_busy();
    let dump_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    let source_path = dump_reply["body"]["sourcePath"].as_str().unwrap();

    // Set two breakpoints
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": source_path },
            "breakpoints": [
                { "line": 1 },
                { "line": 3, "condition": "c > 0" },
            ]
        }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Query debugInfo and verify breakpoints are reported
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 3,
        "command": "debugInfo",
        "arguments": {}
    }));
    frontend.recv_iopub_busy();
    let info_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    assert_eq!(info_reply["success"], true);
    let bp_groups = info_reply["body"]["breakpoints"].as_array().unwrap();

    // Find the group for our source file
    let group = bp_groups
        .iter()
        .find(|g| g["source"].as_str().unwrap().contains("ark-debug-"))
        .expect("No breakpoint group found for dumped cell");

    let bps = group["breakpoints"].as_array().unwrap();
    assert_eq!(bps.len(), 2);
    assert_eq!(bps[0]["line"], 1);
    assert_eq!(bps[1]["line"], 3);
    assert_eq!(bps[1]["condition"], "c > 0");

    // Clean up: clear breakpoints
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 4,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": source_path },
            "breakpoints": []
        }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();
}

#[test]
fn test_notebook_top_level_breakpoint_stops_execution() {
    let frontend = DummyArkFrontendNotebook::lock();

    // A cell with only top-level statements (no enclosing function). Setting a
    // breakpoint on a top-level line must stop execution there.
    let code = "x <- 1\nx <- 2\nx <- 3\nx";

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": code }
    }));
    frontend.recv_iopub_busy();
    let dump_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();
    let source_path = dump_reply["body"]["sourcePath"]
        .as_str()
        .unwrap()
        .to_string();

    // Breakpoint on line 2 (`x <- 2`), a top-level line
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": &source_path },
            "breakpoints": [{ "line": 2 }]
        }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Attach so breakpoints fire
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 3,
        "command": "attach",
        "arguments": { "request": "attach", "type": "notebook" }
    }));
    frontend.recv_iopub_busy();
    let event = frontend.recv_iopub_debug_event();
    assert_eq!(event["event"], "thread");
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Execute the cell — the top-level breakpoint should fire mid-cell.
    frontend.send_execute_request_with_metadata(
        code,
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-top-level" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // The breakpoint verifies when reached, then we stop at it.
    let bp_event = frontend.recv_iopub_debug_event();
    assert_eq!(bp_event["event"], "breakpoint");
    assert_eq!(bp_event["body"]["breakpoint"]["verified"], true);

    let stopped = frontend.recv_iopub_debug_event();
    assert_eq!(stopped["event"], "stopped");

    // Shell reply hasn't arrived — kernel is paused at the breakpoint
    assert!(!frontend.shell_socket.poll_incoming(200).unwrap());

    // Continue — cell runs to completion and returns the final value
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 4,
        "command": "continue",
        "arguments": { "threadId": -1 }
    }));
    frontend.recv_debug_reply();

    frontend.recv_shell_execute_reply();

    let msgs = frontend.recv_iopub_interleaved(&[
        // Control thread: continue debug_request busy/idle
        &[IopubExpectation::BusyControl, IopubExpectation::IdleControl],
        // R thread: continued event, execute result (`[1] 3`), execution idle
        &[
            IopubExpectation::DebugEvent,
            IopubExpectation::ExecuteResult,
            IopubExpectation::IdleShell,
        ],
    ]);
    find_debug_event(&msgs, "continued");

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 5,
        "command": "disconnect",
        "arguments": { "restart": false }
    }));
    frontend.recv_debug_reply();
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
}

#[test]
fn test_notebook_top_level_breakpoint_preserves_invisible_result() {
    let frontend = DummyArkFrontendNotebook::lock();

    // A cell whose last top-level statement is an invisible assignment. After
    // continuing through a breakpoint, the cell must NOT emit a spurious
    // `execute_result` — the bare brace block preserves the final statement's
    // (invisible) visibility.
    let code = "x <- 1\nx <- 2";

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": code }
    }));
    frontend.recv_iopub_busy();
    let dump_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();
    let source_path = dump_reply["body"]["sourcePath"]
        .as_str()
        .unwrap()
        .to_string();

    // Breakpoint on line 1 (`x <- 1`)
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": &source_path },
            "breakpoints": [{ "line": 1 }]
        }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 3,
        "command": "attach",
        "arguments": { "request": "attach", "type": "notebook" }
    }));
    frontend.recv_iopub_busy();
    let event = frontend.recv_iopub_debug_event();
    assert_eq!(event["event"], "thread");
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    frontend.send_execute_request_with_metadata(
        code,
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-invisible" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let bp_event = frontend.recv_iopub_debug_event();
    assert_eq!(bp_event["event"], "breakpoint");

    let stopped = frontend.recv_iopub_debug_event();
    assert_eq!(stopped["event"], "stopped");

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 4,
        "command": "continue",
        "arguments": { "threadId": -1 }
    }));
    frontend.recv_debug_reply();

    frontend.recv_shell_execute_reply();

    // No `ExecuteResult` here: the final statement `x <- 2` is invisible.
    frontend.recv_iopub_interleaved(&[
        &[IopubExpectation::BusyControl, IopubExpectation::IdleControl],
        &[IopubExpectation::DebugEvent, IopubExpectation::IdleShell],
    ]);

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 5,
        "command": "disconnect",
        "arguments": { "restart": false }
    }));
    frontend.recv_debug_reply();
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
}

/// `debugInfo` must hand back the exact `source.path` the frontend sent, not
/// the normalized `FilePath` we key breakpoints on. Here the path carries a `..`
/// segment that `FilePath` collapses, so a normalized echo would differ from
/// what the editor sent and could open a second editor pane on the same file.
#[test]
#[cfg(not(windows))]
fn test_notebook_debug_info_echoes_verbatim_breakpoint_path() {
    let frontend = DummyArkFrontendNotebook::lock();

    // Dump a cell to get a real file on disk that `setBreakpoints` can read.
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": "x <- 1\ny <- 2" }
    }));
    frontend.recv_iopub_busy();
    let dump_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();
    let source_path = dump_reply["body"]["sourcePath"].as_str().unwrap();

    // Build a `..`-variant of that path. It resolves to the same file (the
    // intermediate dir exists), but `FilePath` collapses the `..`, so the
    // normalized key no longer matches these bytes.
    let path = std::path::Path::new(source_path);
    let dir = path.parent().unwrap();
    let dir_base = dir.file_name().unwrap().to_string_lossy();
    let file_name = path.file_name().unwrap().to_string_lossy();
    let sent_path = format!("{}/../{dir_base}/{file_name}", dir.to_string_lossy());
    assert_ne!(sent_path, source_path);

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": sent_path },
            "breakpoints": [{ "line": 1 }]
        }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 3,
        "command": "debugInfo",
        "arguments": {}
    }));
    frontend.recv_iopub_busy();
    let info_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    let bp_groups = info_reply["body"]["breakpoints"].as_array().unwrap();
    let group = bp_groups
        .iter()
        .find(|group| group["source"].as_str() == Some(sent_path.as_str()))
        .expect("debugInfo did not echo the verbatim breakpoint path");
    assert_eq!(group["breakpoints"].as_array().unwrap().len(), 1);
}

#[test]
fn test_notebook_evaluate() {
    let frontend = DummyArkFrontendNotebook::lock();

    let reply = notebook_evaluate(&frontend, 1, "1 + 1");
    assert_eq!(reply["success"], true);
    assert_eq!(reply["command"], "evaluate");
    assert_eq!(reply["body"]["result"], "2");
}

#[test]
fn test_notebook_evaluate_print() {
    let frontend = DummyArkFrontendNotebook::lock();

    // The `/print ` prefix evaluates and returns the captured print output.
    let reply = notebook_evaluate(&frontend, 1, "/print 1:3");
    assert_eq!(reply["success"], true);
    let result = reply["body"]["result"].as_str().unwrap();
    assert!(result.contains("[1] 1 2 3"), "got: {result}");
}

#[test]
fn test_notebook_evaluate_error() {
    let frontend = DummyArkFrontendNotebook::lock();

    // An R error during evaluation comes back as a failed reply, not a crash.
    let reply = notebook_evaluate(&frontend, 1, "stop('boom')");
    assert_eq!(reply["success"], false);
    let message = reply["message"].as_str().unwrap();
    assert!(message.contains("boom"), "got: {message}");

    // The kernel is still responsive afterwards.
    let reply = notebook_evaluate(&frontend, 2, "1 + 1");
    assert_eq!(reply["success"], true);
    assert_eq!(reply["body"]["result"], "2");
}

/// A panic in a try task started from a Control request becomes an error reply
/// rather than aborting the kernel. With the production panic hook installed,
/// a missing recovery boundary on Control would abort the test process.
#[test]
fn test_notebook_try_r_task_panic_is_error_reply() {
    ark::panic::install();

    let frontend = DummyArkFrontendNotebook::lock();

    // Wait for R to reach its first prompt
    notebook_evaluate(&frontend, 1, "1");

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "ark/testPanicTryRTask",
        "arguments": {}
    }));
    frontend.recv_iopub_busy();
    let reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    assert_eq!(reply["success"], false);
    assert_eq!(
        reply["message"],
        "Internal error while running R task: Test panic in a try task"
    );

    // Both the Control thread and the R thread keep serving requests
    let reply = notebook_evaluate(&frontend, 3, "1 + 1");
    assert_eq!(reply["success"], true);
    assert_eq!(reply["body"]["result"], "2");
}

/// An R error escaping a try task started from a Control request becomes an
/// error reply. The error longjumps over the panic boundary that the task
/// entered on the R thread, and that boundary must not outlive the task:
/// otherwise a later panic on the R thread would unwind instead of aborting.
#[test]
#[cfg_attr(not(debug_assertions), ignore = "Requires debug-only test routines")]
fn test_notebook_try_r_task_r_error_is_error_reply() {
    ark::panic::install();

    let frontend = DummyArkFrontendNotebook::lock();

    // Wait for R to reach its first prompt
    notebook_evaluate(&frontend, 1, "1");
    assert_r_thread_aborts_on_panic(&frontend);

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "ark/testRErrorTryRTask",
        "arguments": {}
    }));
    frontend.recv_iopub_busy();
    let reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    assert_eq!(reply["success"], false);
    let message = reply["message"].as_str().unwrap();
    assert!(message.starts_with("Internal error while running R task:"));
    assert!(message.contains("Test R error in a try task"));

    assert_r_thread_aborts_on_panic(&frontend);

    let reply = notebook_evaluate(&frontend, 3, "1 + 1");
    assert_eq!(reply["success"], true);
    assert_eq!(reply["body"]["result"], "2");
}

/// Assert that no panic boundary is active on the R thread at top level.
#[track_caller]
fn assert_r_thread_aborts_on_panic(frontend: &DummyArkFrontendNotebook) {
    frontend.send_execute_request(
        r#".Call("ps_test_recovers_panic")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] FALSE");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// A runaway expression in the watch pane must not freeze the kernel. The
/// evaluation runs on the R main thread, so a `with_timeout()` deadline
/// interrupts it and reports an error rather than blocking forever. The
/// follow-up evaluate checks the interrupt left no stray pending interrupt
/// behind.
/// https://github.com/posit-dev/positron/issues/14481
#[test]
fn test_notebook_evaluate_timeout() {
    let frontend = DummyArkFrontendNotebook::lock();

    // `repeat 1` never returns. The kernel must interrupt it after the timeout.
    let reply = notebook_evaluate(&frontend, 1, "repeat 1");
    assert_eq!(reply["success"], false);
    let message = reply["message"].as_str().unwrap();
    assert!(message.contains("timed out"), "got: {message}");

    // The kernel is still responsive and not stuck on a leftover interrupt.
    let reply = notebook_evaluate(&frontend, 2, "1 + 1");
    assert_eq!(reply["success"], true);
    assert_eq!(reply["body"]["result"], "2");
}

/// The watch pane evaluates expressions while stopped in a frame. A runaway
/// expression there (the canonical bug: `repeat 1`) must time out and leave the
/// debug session intact rather than freezing the kernel: the stopped frame stays
/// usable and execution can still be continued.
/// https://github.com/posit-dev/positron/issues/14481
#[test]
fn test_notebook_watch_pane_infloop_times_out() {
    let frontend = DummyArkFrontendNotebook::lock();

    let fn_code = "fn_watch <- function() {\n  x <- 42\n  x\n}";

    // Dump the cell and set a breakpoint on the return expression (line 3).
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": fn_code }
    }));
    frontend.recv_iopub_busy();
    let dump_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();
    let source_path = dump_reply["body"]["sourcePath"]
        .as_str()
        .unwrap()
        .to_string();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": &source_path },
            "breakpoints": [{ "line": 3 }]
        }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Attach so the breakpoint fires.
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 3,
        "command": "attach",
        "arguments": { "request": "attach", "type": "notebook" }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_iopub_debug_event(); // thread started
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // Define the function.
    frontend.send_execute_request_with_metadata(
        fn_code,
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "watch-def" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let bp_event = frontend.recv_iopub_debug_event();
    assert_eq!(bp_event["event"], "breakpoint");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Call it: stops at the breakpoint, kernel stays busy.
    frontend.send_execute_request_with_metadata(
        "fn_watch()",
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "watch-call" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let stopped = frontend.recv_iopub_debug_event();
    assert_eq!(stopped["event"], "stopped");

    // The frame we're stopped in (what the watch pane targets).
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 4,
        "command": "stackTrace",
        "arguments": { "threadId": -1 }
    }));
    frontend.recv_iopub_busy();
    let stack_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();
    let frame_id = stack_reply["body"]["stackFrames"][0]["id"]
        .as_i64()
        .unwrap();

    // The watch pane evaluates `repeat 1` in the stopped frame. It never returns,
    // so the kernel must interrupt it after the timeout.
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 5,
        "command": "evaluate",
        "arguments": { "expression": "repeat 1", "frameId": frame_id }
    }));
    frontend.recv_iopub_busy();
    let eval_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();
    assert_eq!(eval_reply["success"], false);
    let message = eval_reply["message"].as_str().unwrap();
    assert!(message.contains("timed out"), "got: {message}");

    // The session survived the interrupt: a normal watch expression in the same
    // frame still resolves.
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 6,
        "command": "evaluate",
        "arguments": { "expression": "x", "frameId": frame_id }
    }));
    frontend.recv_iopub_busy();
    let eval_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();
    assert_eq!(eval_reply["success"], true);
    assert_eq!(eval_reply["body"]["result"], "42");

    // And execution can still be continued to completion.
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 7,
        "command": "continue",
        "arguments": { "threadId": -1 }
    }));
    frontend.recv_debug_reply();
    frontend.recv_shell_execute_reply();
    let msgs = frontend.recv_iopub_interleaved(&[
        &[IopubExpectation::BusyControl, IopubExpectation::IdleControl],
        &[
            IopubExpectation::DebugEvent,
            IopubExpectation::ExecuteResult,
            IopubExpectation::IdleShell,
        ],
    ]);
    find_debug_event(&msgs, "continued");

    // Disconnect to reset is_connected for other tests.
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 8,
        "command": "disconnect",
        "arguments": { "restart": false }
    }));
    frontend.recv_debug_reply();
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
}

/// A print method that raises an R error longjumps out of `Rf_PrintValue`. The
/// evaluation must surface that as an error and leave both the try-idle
/// handshake and the `Dap` state intact, so a follow-up evaluate still succeeds.
/// The console (TCP) path is checked in
/// `test_dap_evaluate_erroring_print_does_not_deadlock`.
#[test]
fn test_notebook_evaluate_erroring_print_does_not_deadlock() {
    let frontend = DummyArkFrontendNotebook::lock();

    // Define an S3 object whose print method errors.
    frontend.send_execute_request(
        "print.boom <- function(x, ...) stop('kaboom'); obj <- structure(1, class = 'boom')",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Printing `obj` dispatches `print.boom`, which longjumps out of R.
    let reply = notebook_evaluate(&frontend, 1, "/print obj");
    assert_eq!(reply["success"], false);
    let message = reply["message"].as_str().unwrap();
    assert!(message.contains("kaboom"), "got: {message}");

    // A follow-up evaluate still succeeds: the handshake completed and the `Dap`
    // state is left usable.
    let reply = notebook_evaluate(&frontend, 2, "40 + 2");
    assert_eq!(reply["success"], true);
    assert_eq!(reply["body"]["result"], "42");
}

/// The Watch Pane sends `evaluate` as soon as it gets `stopped`, which the
/// kernel emits before its event loop is running. A single attempt must
/// succeed rather than report "R is busy". This guards the behaviour end to
/// end but doesn't force the request into the gap before the event loop
/// starts.
#[test]
fn test_notebook_evaluate_right_after_stopped() {
    let frontend = DummyArkFrontendNotebook::lock();

    let fn_code = "fn_stopped <- function() {\n  x <- 1\n  x\n}";

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "dumpCell",
        "arguments": { "code": fn_code }
    }));
    frontend.recv_iopub_busy();
    let dump_reply = frontend.recv_debug_reply();
    frontend.recv_iopub_idle();
    let source_path = dump_reply["body"]["sourcePath"]
        .as_str()
        .unwrap()
        .to_string();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 2,
        "command": "setBreakpoints",
        "arguments": {
            "source": { "path": &source_path },
            "breakpoints": [{ "line": 3 }]
        }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 3,
        "command": "attach",
        "arguments": { "request": "attach", "type": "notebook" }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_iopub_debug_event(); // thread started
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    frontend.send_execute_request_with_metadata(
        fn_code,
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "stopped-def" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let bp_event = frontend.recv_iopub_debug_event();
    assert_eq!(bp_event["event"], "breakpoint");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.send_execute_request_with_metadata(
        "fn_stopped()",
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "stopped-call" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let stopped = frontend.recv_iopub_debug_event();
    assert_eq!(stopped["event"], "stopped");

    // No retry: the first attempt must land
    let (reply, _) = notebook_evaluate_once(&frontend, 4, "1 + 1");
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
    assert_eq!(reply["success"], true);
    assert_eq!(reply["body"]["result"], "2");

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 5,
        "command": "continue",
        "arguments": { "threadId": -1 }
    }));
    frontend.recv_debug_reply();
    frontend.recv_shell_execute_reply();
    let msgs = frontend.recv_iopub_interleaved(&[
        &[IopubExpectation::BusyControl, IopubExpectation::IdleControl],
        &[
            IopubExpectation::DebugEvent,
            IopubExpectation::ExecuteResult,
            IopubExpectation::IdleShell,
        ],
    ]);
    find_debug_event(&msgs, "continued");

    // Disconnect to reset is_connected for other tests.
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 6,
        "command": "disconnect",
        "arguments": { "restart": false }
    }));
    frontend.recv_debug_reply();
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
}

/// While R evaluates user code, `evaluate` reports "R is busy" right away
/// instead of waiting for R to reach a prompt.
#[test]
fn test_notebook_evaluate_busy_while_running_code() {
    let frontend = DummyArkFrontendNotebook::lock();

    // Wait for R to reach its first prompt
    notebook_evaluate(&frontend, 1, "1");

    // Keep R busy until we create `release`, however slow the runner is
    let release = std::env::temp_dir().join(format!("ark-evaluate-busy-{}", std::process::id()));
    let release_r = release.to_string_lossy().replace('\\', "/");
    frontend.send_execute_request(
        &format!("while (!file.exists('{release_r}')) Sys.sleep(0.01)"),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let (reply, elapsed) = notebook_evaluate_once(&frontend, 2, "1 + 1");
    std::fs::write(&release, "").unwrap();

    assert_eq!(reply["success"], false);
    assert_eq!(reply["message"], "R is busy");

    // The busy reply doesn't wait out the start timeout of `evaluate`. This is
    // what distinguishes immediate rejection from timing out, so the bound
    // can't be looser than that timeout.
    assert!(elapsed < std::time::Duration::from_millis(200));

    frontend.recv_iopub_interleaved(&[
        &[IopubExpectation::BusyControl, IopubExpectation::IdleControl],
        &[IopubExpectation::IdleShell],
    ]);
    frontend.recv_shell_execute_reply();
    std::fs::remove_file(&release).unwrap();
}

/// Watch expressions run non-interactively, so `readline()` returns `""`
/// instead of requesting input from the frontend.
#[test]
fn test_notebook_evaluate_readline_is_not_interactive() {
    let frontend = DummyArkFrontendNotebook::lock();

    // `notebook_evaluate()` asserts on the busy/idle window, and `readline()`
    // echoes its prompt, so this also checks the echo is captured rather than
    // streamed
    let reply = notebook_evaluate(&frontend, 1, "nchar(readline('prompt> '))");
    assert_eq!(reply["success"], true);
    assert_eq!(reply["body"]["result"], "0L");
}

/// A `browser()` prompt routed to stdin is an input request from the
/// frontend's point of view, so `evaluate` reports "R is busy" right away
/// rather than running code under the prompt.
#[test]
fn test_notebook_evaluate_busy_at_stdin_browser_prompt() {
    let frontend = DummyArkFrontendNotebook::lock();

    frontend.send_execute_request_with_metadata(
        "browser(); 42",
        ExecuteRequestOptions::default(),
        serde_json::json!({ "cellId": "cell-browser-stdin-evaluate" }),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let prompt = frontend.recv_stdin_input_request();
    assert!(
        prompt.contains("Browse"),
        "Expected Browse prompt, got: {prompt}"
    );

    let (reply, elapsed) = notebook_evaluate_once(&frontend, 1, "1 + 1");
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
    assert_eq!(reply["success"], false);
    assert_eq!(reply["message"], "R is busy");
    assert!(elapsed < std::time::Duration::from_millis(200));

    frontend.send_stdin_input_reply(String::from("Q"));
    assert_eq!(frontend.recv_iopub_execute_result(), "[1] 42");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// A single `evaluate` sent on receipt of a notebook execute reply must
/// succeed rather than report "R is busy". Like
/// [`test_notebook_evaluate_right_after_stopped()`], this checks notification
/// ordering end to end without forcing the request into the gap before the
/// event loop starts.
#[test]
fn test_notebook_evaluate_right_after_execute_reply() {
    let frontend = DummyArkFrontendNotebook::lock();

    // Wait for R to reach its first prompt
    notebook_evaluate(&frontend, 1, "1");

    frontend.send_execute_request("x <- 1", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_shell_execute_reply();

    // Do not retry, since that could hide rejection of the first request.
    let (reply, _) = notebook_evaluate_once(&frontend, 2, "1 + 1");
    assert_eq!(reply["success"], true);
    assert_eq!(reply["body"]["result"], "2");

    frontend.recv_iopub_interleaved(&[&[IopubExpectation::IdleShell], &[
        IopubExpectation::BusyControl,
        IopubExpectation::IdleControl,
    ]]);
}

/// A `browser()` entered from a callback in the top-level event loop serves
/// `evaluate`. R reports busy while evaluating a user expression at that
/// prompt and after `continue` while the rest of the callback runs. Returning
/// from the inner prompt must not reopen admission for the outer loop, which
/// is still blocked in the callback. `evaluate` succeeds once that loop resumes.
#[test]
#[cfg_attr(not(debug_assertions), ignore = "Requires debug-only test routines")]
fn test_notebook_evaluate_nested_browser_lifecycle() {
    let frontend = DummyArkFrontendNotebook::lock();

    let entered_expr = MarkerFile::new("nested-browser-entered-expr");
    let release_expr = MarkerFile::new("nested-browser-release-expr");
    let entered_callback = MarkerFile::new("nested-browser-entered-callback");
    let release_callback = MarkerFile::new("nested-browser-release-callback");

    // Attach so the browser is served over DAP rather than routed to stdin
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 1,
        "command": "attach",
        "arguments": { "request": "attach", "type": "notebook" }
    }));
    frontend.recv_iopub_busy();
    frontend.recv_iopub_debug_event(); // thread started
    frontend.recv_debug_reply();
    frontend.recv_iopub_idle();

    // The idle task runs in the top-level event loop once this request
    // completes. Its `browser()` is a nested prompt.
    let callback = format!(
        "{{ browser(); file.create('{entered}'); while (!file.exists('{release}')) Sys.sleep(0.01) }}",
        entered = entered_callback.r_path(),
        release = release_callback.r_path(),
    );
    frontend.send_execute_request(
        &format!(r#"invisible(.Call("ps_test_spawn_eval_idle_task", "{callback}"))"#),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    let stopped = frontend.recv_iopub_debug_event();
    assert_eq!(stopped["event"], "stopped");

    // The nested prompt serves `evaluate`
    let (reply, _) = notebook_evaluate_once(&frontend, 2, "1 + 1");
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
    assert_eq!(reply["success"], true);
    assert_eq!(reply["body"]["result"], "2");

    // Busy while a user expression runs at the nested prompt. Its execute
    // reply is withheld while debugging in notebook mode, so we don't wait
    // for it.
    frontend.send_execute_request(
        &format!(
            "file.create('{entered}'); while (!file.exists('{release}')) Sys.sleep(0.01)",
            entered = entered_expr.r_path(),
            release = release_expr.r_path(),
        ),
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    entered_expr.wait_for();

    assert_notebook_evaluate_busy(&frontend, 3);
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();

    // Back at the nested prompt, which announces itself with `invalidated`
    release_expr.create();
    let invalidated = frontend.recv_iopub_debug_event();
    assert_eq!(invalidated["event"], "invalidated");

    let (reply, _) = notebook_evaluate_once(&frontend, 4, "1 + 1");
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
    assert_eq!(reply["success"], true);
    assert_eq!(reply["body"]["result"], "2");

    // Busy after `continue` while the rest of the callback runs
    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 5,
        "command": "continue",
        "arguments": { "threadId": -1 }
    }));
    frontend.recv_debug_reply();
    let msgs = frontend.recv_iopub_interleaved(&[
        &[IopubExpectation::BusyControl, IopubExpectation::IdleControl],
        &[IopubExpectation::DebugEvent],
    ]);
    find_debug_event(&msgs, "continued");
    entered_callback.wait_for();

    assert_notebook_evaluate_busy(&frontend, 6);
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();

    // No notification marks the callback's return to the top-level event
    // loop, so retry until that loop serves `evaluate` again.
    release_callback.create();
    let reply = notebook_evaluate(&frontend, 7, "1 + 1");
    assert_eq!(reply["success"], true);
    assert_eq!(reply["body"]["result"], "2");

    frontend.send_debug_request(serde_json::json!({
        "type": "request",
        "seq": 8,
        "command": "disconnect",
        "arguments": { "restart": false }
    }));
    frontend.recv_debug_reply();
    frontend.recv_iopub_busy();
    frontend.recv_iopub_idle();
}

/// A temporary file that R code creates to signal the test, or that the test
/// creates to release R code. Removed on drop, including when an assertion
/// fails.
struct MarkerFile {
    path: std::path::PathBuf,
}

impl MarkerFile {
    fn new(name: &str) -> Self {
        let file_name = format!("ark-{name}-{}", std::process::id());
        Self {
            path: std::env::temp_dir().join(file_name),
        }
    }

    /// The path in a form that can be pasted in an R string.
    fn r_path(&self) -> String {
        self.path.to_string_lossy().replace('\\', "/")
    }

    fn create(&self) {
        std::fs::write(&self.path, "").unwrap();
    }

    fn wait_for(&self) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !self.path.exists() {
            if std::time::Instant::now() >= deadline {
                panic!("R never created {}", self.path.display());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl Drop for MarkerFile {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                eprintln!("Can't remove {}: {err:?}", self.path.display());
            }
        }
    }
}

/// Assert that a single `evaluate` reports "R is busy" without waiting out
/// its start timeout.
#[track_caller]
fn assert_notebook_evaluate_busy(frontend: &DummyArkFrontendNotebook, seq: i64) {
    let (reply, elapsed) = notebook_evaluate_once(frontend, seq, "1 + 1");
    assert_eq!(reply["success"], false);
    assert_eq!(reply["message"], "R is busy");
    assert!(elapsed < std::time::Duration::from_millis(200));
}
