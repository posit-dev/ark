//
// stream_filter.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
// Integration tests verifying that debug messages are filtered from console output.
//

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::DummyArkFrontend;

/// Verify that "Called from:" is filtered from console output when browser() is called.
#[test]
fn test_called_from_filtered_at_top_level() {
    let frontend = DummyArkFrontend::lock();

    // Execute browser() which would normally print "Called from: top level"
    frontend.send_execute_request("browser()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Drain any streams - should NOT contain "Called from:"
    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout().contains("Called from:"),
        "Called from: should be filtered from stdout, got: {:?}",
        streams.stdout()
    );

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Exit the debugger
    frontend.execute_request_invisibly("Q");
}

/// Verify that "Called from:" is filtered when browser() is called inside a function.
#[test]
fn test_called_from_filtered_in_function() {
    let frontend = DummyArkFrontend::lock();

    // Define and call a function with browser()
    let code = "local({ browser() })";
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Drain streams - should NOT contain "Called from:"
    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout().contains("Called from:"),
        "Called from: should be filtered from stdout, got: {:?}",
        streams.stdout()
    );

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Exit the debugger
    frontend.execute_request_invisibly("Q");
}

/// Verify that "debug at" is filtered when stepping through sourced code.
#[test]
fn test_debug_at_filtered_when_stepping() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    // Source a file with browser() to enter debug mode
    let file = frontend.send_source(
        "
{
  browser()
  1
  2
}
",
    );
    dap.recv_stopped();

    // Step with `n` which would normally print "debug at file#line: expr".
    // `debug_send_step_command` drains streams internally.
    frontend.debug_send_step_command("n", &file);
    dap.recv_continued();
    dap.recv_stopped();

    // Exit the debugger
    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Verify that PrintValue output from debug messages is suppressed.
/// R's debug handler emits `debug at file#line: ` followed by `PrintValue(Stmt)`.
/// For literals, `PrintValue(42)` produces `[1] 42` which is not valid R syntax.
/// The filter accumulates everything until ReadConsole and suppresses it,
/// so no spurious output leaks to the user.
#[test]
fn test_step_debug_printvalue_suppressed() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let _file = frontend.send_source(
        "
{
  browser()
  42
}
",
    );
    dap.recv_stopped();

    // Step with `n`. The debug message "debug at file#N: [1] 42" is
    // accumulated by the filter and suppressed at the browser prompt.
    // No stdout output should appear for this step.
    frontend.send_execute_request("n", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    dap.recv_continued();
    dap.recv_stopped();

    // Exit the debugger
    frontend.debug_send_quit();
    dap.recv_continued();
}

/// Verify that "debugging in:" and "exiting from:" are filtered.
/// This test uses debug() on a simple function to trigger both messages.
#[test]
fn test_debugging_in_and_exiting_from_filtered() {
    let frontend = DummyArkFrontend::lock();

    // Define a function and debug it
    frontend.execute_request_invisibly("f <- function() 42");
    frontend.execute_request_invisibly("debug(f)");

    // Call the function - this triggers "debugging in:"
    frontend.send_execute_request("f()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Drain streams at this point - should NOT contain "debugging in:"
    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout().contains("debugging in:"),
        "debugging in: should be filtered from stdout, got: {:?}",
        streams.stdout()
    );

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Continue to exit the function - this triggers "exiting from:"
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Drain streams - should NOT contain "exiting from:"
    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout().contains("exiting from:"),
        "exiting from: should be filtered from stdout, got: {:?}",
        streams.stdout()
    );

    // The result [1] 42 should come through
    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Clean up
    frontend.execute_request_invisibly("undebug(f)");
}

/// Verify that normal output is NOT filtered (sanity check).
#[test]
fn test_normal_output_not_filtered() {
    let frontend = DummyArkFrontend::lock();

    // Execute something that produces normal output
    frontend.send_execute_request("cat('Hello, World!\\n')", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // Should see the normal output
    frontend.assert_stream_stdout_contains("Hello, World!");

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// Verify that output during debug sessions that isn't debug chatter passes through.
#[test]
fn test_user_output_in_debug_not_filtered() {
    let frontend = DummyArkFrontend::lock();

    // Enter browser
    frontend.send_execute_request("browser()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Print something in the debug session
    frontend.send_execute_request(
        "cat('User output in debug\\n')",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    // User output should NOT be filtered
    frontend.assert_stream_stdout_contains("User output in debug");

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Exit
    frontend.execute_request_invisibly("Q");
}

// --- Adversarial tests ---
//
// User code produces output resembling debug messages. Outside of debug
// sessions, the filter emits matched content when the next ReadConsole is
// a top-level prompt (not a browser prompt). Inside debug sessions, we
// accept that prefix-matching cat() output is erroneously suppressed
// (documented below as a known limitation).

/// cat() output matching "Called from:" is preserved (IOPub stream path).
#[test]
fn test_adversarial_cat_called_from() {
    let frontend = DummyArkFrontend::lock();
    frontend.send_execute_request(
        r#"cat("Called from: user output\n")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stdout_contains("Called from: user output");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// cat() output matching "debug:" is preserved.
#[test]
fn test_adversarial_cat_debug() {
    let frontend = DummyArkFrontend::lock();
    frontend.send_execute_request(
        r#"cat("debug: user output\n")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stdout_contains("debug: user output");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// cat() output matching "debugging in:" is preserved.
#[test]
fn test_adversarial_cat_debugging_in() {
    let frontend = DummyArkFrontend::lock();
    frontend.send_execute_request(
        r#"cat("debugging in: user output\n")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stdout_contains("debugging in: user output");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// cat() output matching "exiting from:" is preserved.
#[test]
fn test_adversarial_cat_exiting_from() {
    let frontend = DummyArkFrontend::lock();
    frontend.send_execute_request(
        r#"cat("exiting from: user output\n")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stdout_contains("exiting from: user output");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// A print method whose output matches "Called from:" is preserved in the
/// execute_result (autoprint path). Autoprint output goes through the
/// filter and accumulates in `autoprint_output`, so we verify it survives.
#[test]
fn test_adversarial_print_called_from() {
    let frontend = DummyArkFrontend::lock();
    frontend.execute_request_invisibly(
        r#"print.ark_test_adv <- function(x, ...) cat("Called from: custom handler\n")"#,
    );

    frontend.send_execute_request(
        r#"structure(1, class = "ark_test_adv")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let result = frontend.recv_iopub_execute_result();

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    frontend.execute_request_invisibly("rm(print.ark_test_adv)");

    assert!(result.contains("Called from: custom handler"));
}

/// A print method whose output matches "debug:" is preserved in the
/// execute_result (autoprint path).
#[test]
fn test_adversarial_print_debug() {
    let frontend = DummyArkFrontend::lock();
    frontend.execute_request_invisibly(
        r#"print.ark_test_adv2 <- function(x, ...) cat("debug: custom handler\n")"#,
    );

    frontend.send_execute_request(
        r#"structure(1, class = "ark_test_adv2")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let result = frontend.recv_iopub_execute_result();

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    frontend.execute_request_invisibly("rm(print.ark_test_adv2)");

    assert!(result.contains("debug: custom handler"));
}

/// cat() output matching a debug prefix inside a browser session is preserved
/// when the expression part is not valid R (syntax error). The parse-based
/// Known limitation: cat() output matching a non-R-expression debug prefix
/// (CalledFrom, DebuggingIn, ExitingFrom) inside a browser session IS
/// suppressed, even when the text after the prefix is not valid R. We can't
/// distinguish it from real debug output because both are followed by a
/// browser ReadConsole prompt and the filter defers resolution to that point.
#[test]
fn test_adversarial_cat_in_debug_session_is_preserved() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_execute_request("browser()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // User prints prefix-like content while in the browser. Even though
    // "user output in debug" is not valid R, CalledFrom patterns are
    // always deferred to ReadConsole and suppressed at browser prompts.
    frontend.send_execute_request(
        r#"cat("Called from: user output in debug\n")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout().contains("Called from:"),
        "Expected suppression of CalledFrom prefix output in debug session"
    );
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.execute_request_invisibly("Q");
}

/// Known limitation: cat() output matching a debug prefix inside a browser
/// session IS suppressed when the expression part is valid R. We can't
/// distinguish it from real debug output because both are followed by a
/// browser ReadConsole prompt and both parse as valid R.
#[test]
fn test_adversarial_cat_valid_r_in_debug_session_is_suppressed() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_execute_request("browser()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // User prints prefix-like content where the expression part is valid R.
    // Since "x + 1" is valid R, we can't distinguish it from a real debug
    // message, so it's suppressed.
    frontend.send_execute_request(r#"cat("debug: x + 1\n")"#, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout().contains("debug:"),
        "Expected suppression of valid-R prefix-matching output in debug session"
    );
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.execute_request_invisibly("Q");
}

/// Normal output interleaved with prefix-like output from separate cat()
/// calls: all lines are preserved. The prefix-matching line is deferred to
/// ReadConsole (where it's emitted because we're at top level, not in a
/// debug session), so it arrives after the non-matching lines.
#[test]
fn test_adversarial_cat_interleaved_with_normal() {
    let frontend = DummyArkFrontend::lock();
    let code = r#"{
        cat("normal line\n")
        cat("Called from: adversarial line\n")
        cat("another normal line\n")
    }"#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    // "normal line" is emitted immediately (no prefix match).
    // The remaining content is accumulated while the filter is in
    // Filtering state and emitted at ReadConsole (top-level prompt).
    frontend.assert_stream_stdout_contains("normal line");
    frontend.assert_stream_stdout_contains("Called from: adversarial line");
    frontend.assert_stream_stdout_contains("another normal line");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}

/// When stderr arrives while stdout is buffered in the filter (e.g. partial
/// prefix match), the buffered stdout is flushed before the stderr so that
/// ordering is preserved.
#[test]
fn test_stderr_flushes_buffered_stdout() {
    use amalthea::wire::stream::Stream;

    let frontend = DummyArkFrontend::lock();

    // `cat("debug: ")` outputs a partial debug prefix on stdout, which the
    // filter buffers. `cat(file=stderr())` then writes to stderr, which
    // should flush the buffered stdout first.
    let code = r#"{
        cat("debug: ")
        cat("err\n", file = stderr())
    }"#;
    frontend.send_execute_request(code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let streams = frontend.drain_streams();
    let stdout_pos = streams
        .messages
        .iter()
        .position(|(s, t)| *s == Stream::Stdout && t.contains("debug: "));
    let stderr_pos = streams
        .messages
        .iter()
        .position(|(s, t)| *s == Stream::Stderr && t.contains("err"));
    assert!(
        stdout_pos < stderr_pos,
        "Expected stdout before stderr, got stdout at {stdout_pos:?}, stderr at {stderr_pos:?}\n\
         Ordered: {:?}",
        streams.messages
    );

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
}
