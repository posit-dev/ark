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
/// `evaluate` runs through `try_idle_task()`, which waits for the R thread to
/// park in its read-console `Select`. Once R is at a prompt a single attempt
/// lands. The only time it reports "R is busy" here is cold start, before R has
/// reached its first prompt, so we re-send until it's ready. The kernel-side
/// wait paces each attempt, so no sleep is needed. The control thread brackets
/// each attempt with busy/idle on IOPub, and the R-thread evaluation emits
/// nothing there (printed output is captured), so each is the same busy/reply/
/// idle as any other control-only request.
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
    }
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
