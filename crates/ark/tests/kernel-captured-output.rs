//
// kernel-captured-output.rs
//
// Copyright (C) 2026 Posit Software, PBC. All rights reserved.
//
// Integration tests verifying that pending async idle tasks don't swallow
// console output. See the `captured_output` save/restore mechanism in
// `poll_task`.
//
// The flow exercised here:
//
// 1. `.Call("ps_test_spawn_pending_task")` spawns an async idle task that
//    holds a `ConsoleOutputCapture` and awaits a oneshot channel.
//
// 2. On the next event-loop iteration the task is polled, `captured_output`
//    is set, and the future stays `Pending`. `poll_task` saves the capture
//    into `pending_futures` and clears `captured_output`. The task sets
//    the R option `ark.test.task_polled` to `TRUE` so the test can detect
//    when the poll has happened.
//
// 3. The test busy-loops with `getOption("ark.test.task_polled")` until it
//    sees `TRUE`, confirming the idle task has been polled.
//
// 4. A subsequent execute request produces stream output that must reach
//    IOPub (not be swallowed by the suspended capture).
//
// 5. `.Call("ps_test_complete_pending_task")` unblocks the oneshot so the
//    idle task finishes cleanly.

use std::time::Instant;

use amalthea::fixtures::dummy_frontend::ExecuteRequestOptions;
use ark_test::comm::RECV_TIMEOUT;
use ark_test::DummyArkFrontend;

/// Busy-loop until the kernel confirms the pending idle task has been polled
/// by checking the `ark.test.task_polled` R option. Each iteration is a
/// full execute-request round-trip, which also gives the event loop a chance
/// to process the idle task if it hasn't been picked up yet.
fn wait_until_task_polled(frontend: &DummyArkFrontend) {
    let deadline = Instant::now() + RECV_TIMEOUT;
    let polled = std::cell::Cell::new(false);
    loop {
        frontend.execute_request("getOption('ark.test.task_polled', FALSE)", |result| {
            polled.set(result.contains("TRUE"));
        });
        if polled.get() {
            return;
        }
        if Instant::now() >= deadline {
            panic!("Timed out waiting for idle task to be polled");
        }
    }
}

#[test]
fn test_pending_idle_task_does_not_swallow_stdout() {
    let frontend = DummyArkFrontend::lock();

    // Spawn an idle task that holds a ConsoleOutputCapture and stays pending.
    frontend.execute_request_invisibly(r#"invisible(.Call("ps_test_spawn_pending_task"))"#);

    // Wait until the idle task has been polled (capture is active then saved).
    wait_until_task_polled(&frontend);

    // This output must reach IOPub as a stream message, not be captured.
    frontend.send_execute_request(
        r#"cat("hello from stdout\n")"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stdout_contains("hello from stdout");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Clean up: unblock the pending task so it finishes.
    frontend.execute_request_invisibly(r#"invisible(.Call("ps_test_complete_pending_task"))"#);
}

#[test]
fn test_pending_idle_task_does_not_swallow_autoprint() {
    let frontend = DummyArkFrontend::lock();

    // Spawn the pending idle task.
    frontend.execute_request_invisibly(r#"invisible(.Call("ps_test_spawn_pending_task"))"#);

    // Wait until the idle task has been polled.
    wait_until_task_polled(&frontend);

    // Autoprint output must still arrive as execute_result.
    frontend.execute_request("42", |result| {
        assert_eq!(result, "[1] 42");
    });

    // Clean up.
    frontend.execute_request_invisibly(r#"invisible(.Call("ps_test_complete_pending_task"))"#);
}

#[test]
fn test_pending_idle_task_does_not_swallow_stderr() {
    let frontend = DummyArkFrontend::lock();

    // Spawn the pending idle task.
    frontend.execute_request_invisibly(r#"invisible(.Call("ps_test_spawn_pending_task"))"#);

    // Wait until the idle task has been polled.
    wait_until_task_polled(&frontend);

    // Stderr must still reach IOPub.
    frontend.send_execute_request(
        r#"cat("hello from stderr\n", file = stderr())"#,
        ExecuteRequestOptions::default(),
    );
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stderr_contains("hello from stderr");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Clean up.
    frontend.execute_request_invisibly(r#"invisible(.Call("ps_test_complete_pending_task"))"#);
}

#[test]
fn test_multiple_pending_tasks_do_not_swallow_output() {
    let frontend = DummyArkFrontend::lock();

    // Spawn two pending idle tasks. The second spawn overwrites the oneshot
    // sender so the first task will be orphaned (its oneshot is dropped),
    // which is fine - the future resolves with Err(Canceled). The R option
    // is reset by each spawn and set by each poll, so we only need to wait
    // for the last one.
    frontend.execute_request_invisibly(r#"invisible(.Call("ps_test_spawn_pending_task"))"#);
    frontend.execute_request_invisibly(r#"invisible(.Call("ps_test_spawn_pending_task"))"#);

    // Wait until the idle task has been polled.
    wait_until_task_polled(&frontend);

    // Output must still reach IOPub.
    frontend.send_execute_request(r#"cat("still works\n")"#, ExecuteRequestOptions::default());
    frontend.recv_iopub_busy();
    frontend.recv_iopub_execute_input();
    frontend.assert_stream_stdout_contains("still works");
    frontend.recv_iopub_idle();
    frontend.recv_shell_execute_reply();

    // Clean up.
    frontend.execute_request_invisibly(r#"invisible(.Call("ps_test_complete_pending_task"))"#);
}
