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

/// cat() output matching "debug at " is preserved. This prefix has the most
/// complex matching logic (file#line: expr pattern), so it's important to
/// verify it doesn't get swallowed outside debug sessions.
#[test]
fn test_adversarial_cat_debug_at() {
    let frontend = DummyArkFrontend::lock();
    frontend.send_execute_request(
        r#"cat("debug at file.R#1: x <- 1\n")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stdout_contains("debug at file.R#1: x <- 1");
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

/// A print method whose output matches "debug at " is preserved in the
/// execute_result (autoprint path). This is the most complex prefix.
#[test]
fn test_adversarial_print_debug_at() {
    let frontend = DummyArkFrontend::lock();
    frontend.execute_request_invisibly(
        r#"print.ark_test_adv3 <- function(x, ...) cat("debug at file.R#1: x <- 1\n")"#,
    );

    frontend.send_execute_request(
        r#"structure(1, class = "ark_test_adv3")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let result = frontend.recv_iopub_execute_result();

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    frontend.execute_request_invisibly("rm(print.ark_test_adv3)");

    assert!(result.contains("debug at file.R#1: x <- 1"));
}

/// A print method whose output matches "debugging in:" is preserved in the
/// execute_result (autoprint path).
#[test]
fn test_adversarial_print_debugging_in() {
    let frontend = DummyArkFrontend::lock();
    frontend.execute_request_invisibly(
        r#"print.ark_test_adv4 <- function(x, ...) cat("debugging in: custom handler\n")"#,
    );

    frontend.send_execute_request(
        r#"structure(1, class = "ark_test_adv4")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let result = frontend.recv_iopub_execute_result();

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    frontend.execute_request_invisibly("rm(print.ark_test_adv4)");

    assert!(result.contains("debugging in: custom handler"));
}

/// A print method whose output matches "exiting from:" is preserved in the
/// execute_result (autoprint path). This prefix is specifically handled by
/// `strip_leading_debug_lines`, which is gated on `is_browser ||
/// debug_was_debugging`, so at top level it must not fire.
#[test]
fn test_adversarial_print_exiting_from() {
    let frontend = DummyArkFrontend::lock();
    frontend.execute_request_invisibly(
        r#"print.ark_test_adv5 <- function(x, ...) cat("exiting from: custom handler\n")"#,
    );

    frontend.send_execute_request(
        r#"structure(1, class = "ark_test_adv5")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let result = frontend.recv_iopub_execute_result();

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    frontend.execute_request_invisibly("rm(print.ark_test_adv5)");

    assert!(result.contains("exiting from: custom handler"));
}

/// A print method whose output starts with "exiting from:" followed by real
/// content survives at top level. Guards against `strip_leading_debug_lines`
/// being accidentally called without the `is_browser || debug_was_debugging`
/// gate.
#[test]
fn test_adversarial_print_leading_exiting_from_survives() {
    let frontend = DummyArkFrontend::lock();
    frontend.execute_request_invisibly(
        r#"print.ark_test_strip1 <- function(x, ...) cat("exiting from: g()\nreal result\n")"#,
    );

    frontend.send_execute_request(
        r#"structure(1, class = "ark_test_strip1")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let result = frontend.recv_iopub_execute_result();

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    frontend.execute_request_invisibly("rm(print.ark_test_strip1)");

    assert!(result.contains("exiting from: g()"));
    assert!(result.contains("real result"));
}

/// A print method whose output has normal text followed by a debug prefix
/// line survives at top level. Guards against `truncate_at_debug_prefix`
/// being accidentally called without the `is_browser` gate.
#[test]
fn test_adversarial_print_trailing_debug_prefix_survives() {
    let frontend = DummyArkFrontend::lock();
    frontend.execute_request_invisibly(
        r#"print.ark_test_trunc1 <- function(x, ...) cat("user output\ndebug at file.R#1: x <- 1\nmore output\n")"#,
    );

    frontend.send_execute_request(
        r#"structure(1, class = "ark_test_trunc1")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let result = frontend.recv_iopub_execute_result();

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    frontend.execute_request_invisibly("rm(print.ark_test_trunc1)");

    assert!(result.contains("user output"));
    assert!(result.contains("debug at file.R#1: x <- 1"));
    assert!(result.contains("more output"));
}

/// cat() output matching a debug prefix inside a browser session is preserved
/// when the expression part is not valid R (syntax error). The parse-based
/// Known limitation: cat() output matching a non-R-expression debug prefix
/// (CalledFrom, DebuggingIn, ExitingFrom) inside a browser session IS
/// suppressed, even when the text after the prefix is not valid R. We can't
/// distinguish it from real debug output because both are followed by a
/// browser ReadConsole prompt and the filter defers resolution to that point.
#[test]
fn test_adversarial_cat_in_debug_session_is_suppressed() {
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

/// Verify that multi-line PrintValue output from debug stepping is fully
/// suppressed while user output from `print()` is preserved.
///
/// When stepping through `print(20)` in a top-level braced expression:
/// - `print(20)` output `[1] 20` goes through the stream filter (n_frame=2,
///   so `is_auto_printing()` is false) and arrives as IOPub stream stdout.
/// - The `debug at #N: list(...)` message for the NEXT expression goes
///   through autoprint (n_frame=0) and is removed by
///   `strip_debug_prefix_lines`, so no execute_result is emitted.
#[test]
fn test_multiline_printvalue_truncated_from_autoprint() {
    let frontend = DummyArkFrontend::lock();

    // Create a long list that will print across multiple lines.
    let long_string = "x".repeat(60);
    let code = format!(
        r#"{{
  browser()
  print(20)
  list(
    "{long_string}",
    "{long_string}",
    "{long_string}"
  )
}}"#
    );

    frontend.send_execute_request(&code, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Step to print(20) - emits "debug at #N: print(20)" which is truncated
    frontend.send_execute_request("n", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Step through print(20): executes print(20) producing "[1] 20" on
    // IOPub stream (n_frame=2), then R advances to list(...) emitting
    // "debug at #N: list(long...)" into autoprint (n_frame=0).
    // `strip_debug_prefix_lines` truncates the debug content so no
    // execute_result is emitted. If it leaked, `recv_iopub_idle()` would
    // see an unexpected execute_result and panic.
    frontend.send_execute_request("n", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stdout_contains("[1] 20");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Exit the debugger
    frontend.execute_request_invisibly("Q");
}

/// When `c` (continue) is used in a debugged function, user cat() output
/// matching a debug prefix is deferred by the stream filter but ultimately
/// emitted, because `filter.set_debugging(false)` fires when the browser's
/// ReadConsole returns "c". So during the function body, `was_debugging` is
/// false, and at the top-level ReadConsole the filter emits the content
/// (was_debugging=false, is_browser=false).
///
/// `exiting from: f()` reaches autoprint at n_frame=0. Since there's a
/// return value after it, `strip_leading_debug_lines` keeps everything
/// (noise + result) to avoid losing user content.
#[test]
fn test_continue_prefix_cat_preserved() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request_invisibly(
        r#"f <- function() {
            cat("line1\n")
            cat("Called from: user log\n")
            cat("line3\n")
            1
        }"#,
    );
    frontend.execute_request_invisibly("debug(f)");

    // Call f() - enters debugger at first expression
    frontend.send_execute_request("f()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Continue with "c" - executes entire function body without stopping.
    // `filter.set_debugging(false)` fires when the browser's ReadConsole
    // returns "c", so all cat() output during execution has was_debugging=false.
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let streams = frontend.drain_streams();

    // All three cat() lines survive: the prefix-matching line is deferred
    // by the stream filter but emitted at the top-level ReadConsole because
    // was_debugging=false and is_browser=false.
    assert!(
        streams.stdout().contains("line1"),
        "line1 should survive, got: {:?}",
        streams.stdout()
    );
    assert!(
        streams.stdout().contains("Called from: user log"),
        "prefix-matching cat output should survive with c, got: {:?}",
        streams.stdout()
    );
    assert!(
        streams.stdout().contains("line3"),
        "line3 should survive, got: {:?}",
        streams.stdout()
    );

    let result = frontend.recv_iopub_execute_result();
    assert!(result.contains("[1] 1"));
    // Note: exiting from: is kept (noise) because there's a result after it.
    // This is the conservative approach to avoid losing user content.
    assert!(
        result.contains("exiting from:"),
        "exiting from: should be kept along with result, got: {result:?}"
    );

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.execute_request_invisibly("undebug(f)");
}

/// Known limitation: when `c` and another expression are submitted as a
/// batch (e.g., `"c\n1 + 1"`), `exiting from:` leaks through the stream
/// path instead of being suppressed.
///
/// This happens because `filter.set_debugging(false)` fires in the cleanup
/// when the browser's ReadConsole returns `"c"`, before R emits
/// `"exiting from:"`. So the filter records `was_debugging = false`. At
/// the next ReadConsole (top-level, for `"1 + 1"`), `is_browser` is also
/// false, so the filter emits the content instead of suppressing it.
///
/// Fixing this would require mirroring `debug_was_debugging` in the filter,
/// but that would also suppress user `cat()` output matching a prefix
/// during `"c"` execution, regressing `test_continue_prefix_cat_preserved`.
#[test]
fn test_known_limitation_exiting_from_leaks_in_batch() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request_invisibly("f <- function() 42");
    frontend.execute_request_invisibly("debug(f)");

    // Call f() - enters debugger
    frontend.send_execute_request("f()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Send "c" and "1 + 1" as a batch. The kernel parses this as two
    // expressions. "c" is recognised as a debug command and forwarded to
    // the browser. "1 + 1" stays in pending_inputs.
    //
    // Because pending_inputs is non-empty when R emits "exiting from:",
    // the message goes through the stream filter (not autoprint). The
    // filter has is_debugging=false (set in cleanup), so it leaks.
    frontend.send_execute_request("c\n1 + 1", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let streams = frontend.drain_streams();
    assert!(
        streams.stdout().contains("exiting from:"),
        "Expected 'exiting from:' to leak in batch case, got: {:?}",
        streams.stdout()
    );

    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.execute_request_invisibly("undebug(f)");
}

/// When `exiting from:` has a return value after it, we keep everything
/// (noise + result) to avoid losing user content. This test verifies the
/// result is preserved even with multi-line debug output.
#[test]
fn test_exiting_from_multiline_with_result_kept() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request_invisibly(
        "f <- function(long_argument_1 = 1, long_argument_2 = 2, long_argument_3 = 3, long_argument_4 = 4) 42",
    );
    frontend.execute_request_invisibly("debug(f)");

    frontend.send_execute_request(
        "f(long_argument_1 = 1, long_argument_2 = 2, long_argument_3 = 3, long_argument_4 = 4)",
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Continue - R emits multi-line "exiting from:" then the return value
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout().contains("exiting from:"),
        "exiting from: should not be in stdout (goes to autoprint), got: {:?}",
        streams.stdout()
    );

    let result = frontend.recv_iopub_execute_result();
    assert!(result.contains("[1] 42"));
    // Note: exiting from: is kept (noise) because there's a result after it.
    // This is the conservative approach to avoid losing user content.
    assert!(
        result.contains("exiting from:"),
        "exiting from: should be kept along with result, got: {result:?}"
    );

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.execute_request_invisibly("undebug(f)");
}

/// Verify that the `debug: ` prefix (without "at") is filtered when stepping
/// through un-sourced code. Sourced code produces `debug at file#line: `,
/// but un-sourced code produces `debug: expr`.
#[test]
fn test_debug_prefix_filtered_when_stepping_unsourced() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request_invisibly("f <- function() { x <- 1; x }");
    frontend.execute_request_invisibly("debug(f)");

    // Call f() - triggers "debugging in: f()" and "debug: x <- 1"
    frontend.send_execute_request("f()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout().contains("debugging in:"),
        "debugging in: should be filtered, got: {:?}",
        streams.stdout()
    );
    assert!(
        !streams.stdout().contains("debug: "),
        "debug: prefix should be filtered, got: {:?}",
        streams.stdout()
    );

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Step with n - triggers "debug: x"
    frontend.send_execute_request("n", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout().contains("debug: "),
        "debug: prefix should be filtered on step, got: {:?}",
        streams.stdout()
    );

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Continue to exit
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout().contains("exiting from:"),
        "exiting from: should be filtered, got: {:?}",
        streams.stdout()
    );

    frontend.recv_iopub_execute_result();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.execute_request_invisibly("undebug(f)");
}

/// Known limitation: when a prefix-matching cat() is followed by a
/// non-matching cat() in the same expression inside a browser session,
/// both are suppressed. The first cat() puts the filter in Filtering state,
/// which accumulates all subsequent content until ReadConsole. At the
/// browser prompt, everything is suppressed.
#[test]
fn test_collateral_suppression_in_browser() {
    let frontend = DummyArkFrontend::lock();

    frontend.send_execute_request("browser()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // First cat() matches a prefix, second does not. Both are suppressed
    // because the Filtering state accumulates everything until ReadConsole.
    frontend.send_execute_request(
        r#"{ cat("debug: log msg\n"); cat("innocent line\n") }"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout().contains("debug: log msg"),
        "prefix-matching cat should be suppressed in browser, got: {:?}",
        streams.stdout()
    );
    assert!(
        !streams.stdout().contains("innocent line"),
        "collateral cat output should also be suppressed in browser, got: {:?}",
        streams.stdout()
    );
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    frontend.execute_request_invisibly("Q");
}

/// When in a browser, calling a debugged function and continuing produces
/// `exiting from:` before the return value. Since there's a return value,
/// we keep everything (noise + result) to avoid losing user content.
#[test]
fn test_exiting_from_kept_with_result_at_browser_prompt() {
    let frontend = DummyArkFrontend::lock();

    frontend.execute_request_invisibly("f <- function() 42");
    frontend.execute_request_invisibly("debug(f)");

    // Enter a browser
    frontend.send_execute_request("browser()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Call f() from the browser - enters f's debugger
    frontend.send_execute_request("f()", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.drain_streams();
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Continue from f's debugger. R emits "exiting from: f()" then the
    // return value. Since there's a result, we keep everything (noise + result).
    frontend.send_execute_request("c", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();

    let streams = frontend.drain_streams();
    assert!(
        !streams.stdout().contains("exiting from:"),
        "exiting from: should not be in stdout (goes to autoprint), got: {:?}",
        streams.stdout()
    );

    let result = frontend.recv_iopub_execute_result();
    // Note: exiting from: is kept (noise) because there's a result after it.
    assert!(
        result.contains("exiting from:"),
        "exiting from: should be kept along with result, got: {result:?}"
    );
    assert!(result.contains("[1] 42"));

    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Exit the browser
    frontend.execute_request_invisibly("Q");
    frontend.execute_request_invisibly("undebug(f)");
}

/// Verify that user output from cat() is preserved between debug steps.
/// When stepping through sourced code with cat() calls, the user output
/// should survive while debug messages are filtered.
#[test]
fn test_user_output_preserved_between_debug_steps() {
    let frontend = DummyArkFrontend::lock();
    let mut dap = frontend.start_dap();

    let file = frontend.send_source(
        "
{
  browser()
  cat('step one output\\n')
  cat('step two output\\n')
  1
}
",
    );
    dap.recv_stopped();

    // Step to first cat()
    frontend.debug_send_step_command("n", &file);
    dap.recv_continued();
    dap.recv_stopped();

    // Execute first cat() - user output should appear
    frontend.send_execute_request("n", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();
    frontend.assert_stream_stdout_contains("step one output");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    dap.recv_continued();
    dap.recv_stopped();

    // Execute second cat() - user output should also appear
    frontend.send_execute_request("n", ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.recv_iopub_stop_debug();
    frontend.recv_iopub_start_debug();
    frontend.assert_stream_stdout_contains("step two output");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();
    dap.recv_continued();
    dap.recv_stopped();

    // Exit the debugger
    frontend.debug_send_quit();
    dap.recv_continued();
}
